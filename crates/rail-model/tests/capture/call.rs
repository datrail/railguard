use rail_model::capture::{
    ContextKey, KeyReading, ObservedCall, PathReading, Taken, Untaken, compile_plan, parse_plan,
};
use rail_model::condition::{DEFAULT_STEP_LIMIT, Refusal};
use serde_json::{Map, Value, json};

use super::patch;
use crate::keep_body;

const PLANS: &str = include_str!("../../../../vectors/capture/plans.json");

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
    let plans: Value = serde_json::from_str(PLANS).unwrap();
    let mut plan = plans["plans"]["matching"].clone();
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
    let plans: Value = serde_json::from_str(PLANS).unwrap();
    let mut plan = plans["plans"]["matching"].clone();
    let tags = json!({"path": ["request", "body", "tags"], "op": "contains", "value": "x"});
    patch(
        &mut plan,
        &json!({"op": "add", "path": "/entries/2/match/-", "value": tags}),
    );
    let user_key =
        json!({"name": "user", "path": ["request", "body", "user"], "field_type": "string"});
    patch(
        &mut plan,
        &json!({"op": "add", "path": "/entries/2/record/context_keys/-", "value": user_key}),
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
