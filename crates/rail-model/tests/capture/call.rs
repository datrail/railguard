use std::sync::Arc;

use rail_model::body::{BodyBuffer, BodyRead, ByteBudget, Unparsed, Unread};
use rail_model::capture::{
    CompiledPlan, ContextKey, KeptValue, KeyReading, Measure, MeasureReading, ObservedCall,
    PathReading, Replacement, Taken, Untaken, compile_plan, parse_plan,
};
use rail_model::condition::{BodyUnread, DEFAULT_STEP_LIMIT, Refusal};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::patch;

const VECTORS: &str = include_str!("../../../../vectors/capture_call_conformance_cases.json");

#[derive(Deserialize)]
struct Vectors {
    plan: Value,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    patch: Vec<Value>,
    call: Call,
    expect: Value,
}

#[derive(Deserialize)]
struct Call {
    authority: String,
    method: String,
    path: String,
    body: Value,
    body_bytes: Option<u64>,
}

fn observe(call: &Call) -> ObservedCall {
    let (body, body_bytes) = match &call.body {
        Value::String(text) => (keep_body(text.as_bytes()), text.len() as u64),
        unread => {
            let why = match unread["unread"].as_str().unwrap() {
                "not_sent" => Unread::NotSent,
                "cut_short" => Unread::CutShort,
                "past_cap" => Unread::PastCap,
                "past_budget" => Unread::PastBudget,
                other => panic!("{other}"),
            };
            (BodyRead::Unread(why), 0)
        }
    };
    ObservedCall {
        authority: call.authority.clone().into_bytes(),
        method: call.method.clone().into_bytes(),
        path: call.path.clone().into_bytes(),
        body,
        body_bytes: call.body_bytes.unwrap_or(body_bytes),
    }
}

fn keep_body(body: &[u8]) -> BodyRead {
    let mut buffer = BodyBuffer::new(usize::MAX, &ByteBudget::new(usize::MAX));
    buffer.append(body);
    BodyRead::Whole(Arc::new(buffer.finish().unwrap()))
}

fn snake_case<T: std::fmt::Debug>(value: T) -> String {
    let mut name = String::new();
    for (at, character) in format!("{value:?}").chars().enumerate() {
        if character.is_uppercase() && at > 0 {
            name.push('_');
        }
        name.extend(character.to_lowercase());
    }
    name
}

fn describe_body_unread(why: BodyUnread) -> String {
    match why {
        BodyUnread::Unread(why) => snake_case(why),
        BodyUnread::Unparsed(Unparsed::Malformed) => "not_json".to_owned(),
        BodyUnread::Unparsed(Unparsed::PastBudget) => "tree_past_budget".to_owned(),
        BodyUnread::Batch => "batch".to_owned(),
    }
}

fn describe_reading<T>(reading: &PathReading<T>, found: impl Fn(&T) -> Value) -> Value {
    match reading {
        PathReading::Found(value) => found(value),
        PathReading::Absent => json!("absent"),
        PathReading::Ambiguous => json!("ambiguous"),
        PathReading::BodyUnread(why) => json!({"body_unread": describe_body_unread(*why)}),
        PathReading::Refused(refusal) => json!({"refused": refusal}),
    }
}

fn describe_kept(value: &KeptValue) -> Value {
    match value {
        KeptValue::String(text) => json!({"string": text}),
        KeptValue::Replaced(Replacement::KeyShape) => json!({"replaced": "key_shape"}),
        KeptValue::Replaced(Replacement::PastLongest) => json!({"replaced": "past_longest"}),
        KeptValue::NotAString => json!({"not_a_string": null}),
    }
}

fn describe(compiled: &CompiledPlan, call: &ObservedCall) -> Value {
    let captured = compiled.capture(call);
    let (entry, record) = match captured.taken {
        Taken::Entry { entry, record } => (entry, record),
        Taken::CatchAll(why) => {
            let why = match why {
                Untaken::NoEntry => "no_entry".to_owned(),
                Untaken::Unmatchable(why) => format!("unmatchable: {}", snake_case(why)),
                Untaken::Refused(entry, refusal) => {
                    format!("refused {}: {}", entry.id, json!(refusal).as_str().unwrap())
                }
                Untaken::Ambiguous(entry) => format!("ambiguous {}", entry.id),
                Untaken::BodyUnread(entry, why) => {
                    format!("body_unread {}: {}", entry.id, describe_body_unread(why))
                }
            };
            return json!({"taken": "catch_all", "why": why});
        }
    };
    let mut keys = Map::new();
    for (key, reading) in entry.record.context_keys.iter().zip(&record.context_keys) {
        let name = match key {
            ContextKey::Method => "method".to_owned(),
            ContextKey::Named { name, .. } => name.clone(),
        };
        let reading = match reading {
            KeyReading::Method(method) => json!({"method": method.as_str()}),
            KeyReading::Path(reading) => describe_reading(reading, describe_kept),
        };
        keys.insert(name, reading);
    }
    let mut measures = Map::new();
    for (measure, reading) in entry.record.measures.iter().zip(&record.measures) {
        let name = match measure {
            Measure::Named { name, .. } => name.as_str(),
            _ => "request_bytes",
        };
        match reading {
            MeasureReading::CallCount | MeasureReading::ErrorCount => {}
            MeasureReading::RequestBytes(bytes) => drop(measures.insert(name.into(), json!(bytes))),
            MeasureReading::Size(paths) => {
                let paths = paths
                    .iter()
                    .map(|path| describe_reading(path, |bytes| json!({"found": bytes})))
                    .collect();
                measures.insert(name.into(), Value::Array(paths));
            }
        }
    }
    assert_eq!(record.context_keys.len(), entry.record.context_keys.len());
    assert_eq!(record.measures.len(), entry.record.measures.len());
    json!({"taken": entry.id, "keys": keys, "measures": measures})
}

fn compile(vectors: &Vectors, case: &Case) -> CompiledPlan {
    let mut plan = vectors.plan.clone();
    case.patch
        .iter()
        .for_each(|operation| patch(&mut plan, operation));
    compile_plan(parse_plan(&plan.to_string()).unwrap())
}

#[test]
fn test_vectors() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());
    let mut failures = Vec::new();
    for case in &vectors.cases {
        let compiled = compile(&vectors, case);
        let found = describe(&compiled, &observe(&case.call));
        if found != case.expect {
            failures.push(format!("{}: {found}", case.name));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn search_code(arguments: &Value) -> ObservedCall {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "search_code", "arguments": arguments}});
    let body = body.to_string();
    ObservedCall {
        authority: b"api.github.com".to_vec(),
        method: b"POST".to_vec(),
        path: b"/mcp".to_vec(),
        body_bytes: body.len() as u64,
        body: keep_body(body.as_bytes()),
    }
}

// A body's list too long to check stops the walk at the entry checking it, for that call only.
#[test]
fn test_spending_the_step_budget_stops_the_walk() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    let mut plan = vectors.plan.clone();
    let tags = json!({"path": ["request", "body", "params", "arguments", "tags"], "op": "contains", "value": "x"});
    patch(
        &mut plan,
        &json!({"op": "add", "path": "/entries/0/match/-", "value": tags}),
    );
    // e1's tool, so only the list stands between it and the call.
    let tool = "/entries/1/match/3/value";
    patch(
        &mut plan,
        &json!({"op": "replace", "path": tool, "value": "create_or_update_file"}),
    );
    patch(
        &mut plan,
        &json!({"op": "replace", "path": "/entries/0/match/3/value", "value": "search_code"}),
    );
    let compiled = compile_plan(parse_plan(&plan.to_string()).unwrap());
    let within = vec![json!("y"); (DEFAULT_STEP_LIMIT / 2) as usize];
    let past = vec![json!("y"); DEFAULT_STEP_LIMIT as usize];
    let taken = |tags: Vec<Value>| compiled.capture(&search_code(&json!({"tags": tags}))).taken;
    assert!(matches!(taken(within), Taken::CatchAll(Untaken::NoEntry)));
    assert!(matches!(
        taken(past),
        Taken::CatchAll(Untaken::Refused(entry, Refusal::BudgetExceeded)) if entry.id == "e1"
    ));
    // The next call has a budget of its own.
    assert!(matches!(
        taken(vec![json!("x")]),
        Taken::Entry { entry, .. } if entry.id == "e1"
    ));
}
