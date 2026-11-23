use rail_model::body::{BodyRead, Unparsed, Unread};
use rail_model::capture::{
    CompiledPlan, ContextKey, KeptValue, KeyReading, Measure, MeasureReading, ObservedCall,
    PathReading, Replacement, Taken, Untaken, compile_plan, parse_plan,
};
use rail_model::condition::{BodyUnread, DEFAULT_STEP_LIMIT, Refusal};
use rail_model::request_line::HostReading;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::patch;
use crate::keep_body;

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

fn describe_untaken(why: &Untaken) -> String {
    match why {
        Untaken::NoEntry => "no_entry".to_owned(),
        Untaken::Unmatchable(why) => format!("unmatchable: {}", snake_case(why)),
        Untaken::Refused(entry, refused) => {
            let refusal = json!(refused.refusal);
            format!("refused {}: {}", entry.id, refusal.as_str().unwrap())
        }
        Untaken::Stopped(entry, refusal) => {
            format!("stopped {}: {}", entry.id, json!(refusal).as_str().unwrap())
        }
        Untaken::Ambiguous(entry) => format!("ambiguous {}", entry.id),
        Untaken::BodyUnread(entry, why) => {
            format!("body_unread {}: {}", entry.id, describe_body_unread(*why))
        }
    }
}

fn describe_key(key: &ContextKey, reading: &KeyReading) -> (String, Value) {
    let name = match key {
        ContextKey::Method => "method",
        ContextKey::Host => "host",
        ContextKey::PathPrefix { .. } => "path_prefix",
        ContextKey::Operation => "operation",
        ContextKey::Named { name, .. } => name,
    };
    let reading = match reading {
        KeyReading::Method(method) => json!({"method": method.as_str()}),
        KeyReading::Host(HostReading::Read(host)) => json!({"host": host}),
        KeyReading::Host(unparseable) => json!({"unparseable": snake_case(unparseable)}),
        KeyReading::PathPrefix(prefix) => json!({"path_prefix": prefix}),
        KeyReading::Operation(reading) | KeyReading::Path(reading) => {
            describe_reading(reading, describe_kept)
        }
    };
    (name.to_owned(), reading)
}

fn describe(compiled: &CompiledPlan, call: &ObservedCall) -> Value {
    let captured = compiled.capture(call);
    let (taken, record) = match &captured.taken {
        Taken::Entry(entry) => (json!({"entry": entry.id}), &entry.record),
        Taken::CatchAll(catch_all, why) => (
            json!({"catch_all": catch_all.id, "why": describe_untaken(why)}),
            &catch_all.record,
        ),
    };
    let reading = &captured.record;
    assert_eq!(reading.context_keys.len(), record.context_keys.len());
    assert_eq!(reading.measures.len(), record.measures.len());
    let keys: Map<String, Value> = record
        .context_keys
        .iter()
        .zip(&reading.context_keys)
        .map(|(key, reading)| describe_key(key, reading))
        .collect();
    let mut measures = Map::new();
    for (measure, reading) in record.measures.iter().zip(&reading.measures) {
        let name = match measure {
            Measure::Named { name, .. } => name.as_str(),
            _ => "request_bytes",
        };
        match reading {
            MeasureReading::CallCount | MeasureReading::ErrorCount => {}
            MeasureReading::RequestBytes(bytes) => drop(measures.insert(name.into(), json!(bytes))),
            MeasureReading::SizeInBytes(paths) => {
                let paths = paths
                    .iter()
                    .map(|path| describe_reading(path, |bytes| json!({"found": bytes})))
                    .collect();
                measures.insert(name.into(), Value::Array(paths));
            }
        }
    }
    json!({"taken": taken, "keys": keys, "measures": measures})
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
    assert!(matches!(
        taken(within),
        Taken::CatchAll(_, Untaken::NoEntry)
    ));
    assert!(matches!(
        taken(past),
        Taken::CatchAll(_, Untaken::Stopped(entry, Refusal::BudgetExceeded)) if entry.id == "e1"
    ));
    // The next call has a budget of its own.
    assert!(matches!(
        taken(vec![json!("x")]),
        Taken::Entry(entry) if entry.id == "e1"
    ));
}

// The budget a call's matching leaves is its record's: a key passing many members is read alone,
// and refused after a condition passing a long list.
#[test]
fn test_a_record_reads_within_the_budget_its_matching_left() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    let mut plan = vectors.plan.clone();
    let tags = json!({"path": ["request", "body", "tags"], "op": "contains", "value": "x"});
    patch(
        &mut plan,
        &json!({"op": "add", "path": "/entries/2/match/-", "value": tags}),
    );
    let compiled = compile_plan(parse_plan(&plan.to_string()).unwrap());
    // Matching passes the members and the list, then the record the members again.
    let (members, tags) = (DEFAULT_STEP_LIMIT * 3 / 10, DEFAULT_STEP_LIMIT / 2);
    let call = |tags: u64, members: u64| {
        let mut body: Map<String, Value> = (0..members)
            .map(|at| (format!("k{at}"), json!(at)))
            .collect();
        let mut tags = vec![json!("y"); tags as usize];
        tags.push(json!("x"));
        body.insert("tags".into(), Value::Array(tags));
        let body = Value::Object(body).to_string();
        ObservedCall {
            authority: b"hooks.slack.com".to_vec(),
            method: b"POST".to_vec(),
            path: b"/services/T0ABC".to_vec(),
            body_bytes: body.len() as u64,
            body: keep_body(body.as_bytes()),
        }
    };
    let user = |call: &ObservedCall| {
        let captured = compiled.capture(call);
        let (Taken::Entry(entry), record) = (captured.taken, captured.record) else {
            panic!("not taken");
        };
        let at = entry
            .record
            .context_keys
            .iter()
            .position(|key| matches!(key, ContextKey::Named { name, .. } if name == "user"));
        record.context_keys.into_iter().nth(at.unwrap()).unwrap()
    };
    assert_eq!(
        user(&call(0, members)),
        KeyReading::Path(PathReading::Absent)
    );
    assert_eq!(
        user(&call(tags, members)),
        KeyReading::Path(PathReading::Refused(Refusal::BudgetExceeded))
    );
}
