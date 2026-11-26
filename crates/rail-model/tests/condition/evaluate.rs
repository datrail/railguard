use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rail_model::body::BodyRead;
use rail_model::condition::{
    Budget, Condition, ConditionUse, Environment, Predicate, Refusal, Verdict, compile_condition,
    evaluate_condition,
};
use rail_model::endpoint::Endpoint;
use rail_model::request::{Headers, ObservedRequest};
use serde_json::{Value, json};

use crate::{keep_body, observed};

fn evaluate_on_header(condition: Value, value: &str, budget: &mut Budget) -> Verdict {
    let condition: Condition = serde_json::from_value(condition).unwrap();
    let headers = Headers::from_lines([("x-a", value)]);
    let environment = Environment::for_judging(None, &Endpoint::Keyless, &headers);
    evaluate_condition(
        &compile_condition(&condition, ConditionUse::GatewayPolicy),
        &environment,
        budget,
    )
    .verdict
}

#[test]
fn test_refuses_past_the_step_limit_and_not_before() {
    let condition = json!({"path": "request.headers.x-a", "op": "eq", "value": "a"});

    // One step for the condition, and one for each of the three values.
    assert_eq!(
        evaluate_on_header(condition.clone(), "a,a,a", &mut Budget::new(4)),
        Verdict::Holds
    );
    assert_eq!(
        evaluate_on_header(condition, "a,a,a", &mut Budget::new(3)),
        Verdict::Refused(Refusal::BudgetExceeded)
    );
}

#[test]
fn test_refuses_an_in_or_a_match_past_the_step_limit_and_not_before() {
    let is_in = json!({"path": "request.headers.x-a", "op": "in", "value": ["b", "c", "a"]});
    let matches = json!({"path": "request.headers.x-a", "op": "matches", "value": "a*a"});
    let long = "a".repeat(200);
    let cases = [
        // The condition, then one per operand item.
        (&is_in, "a".to_owned(), 4),
        // The condition, one per value, then each value against each operand item.
        (&is_in, "a,a".to_owned(), 9),
        // The condition, then one per scan and one per 64 bytes of value and pattern.
        (&matches, long.clone(), 5),
        (&matches, format!("b,{long}"), 8),
    ];
    for (condition, value, steps) in cases {
        let at = |steps| evaluate_on_header(condition.clone(), &value, &mut Budget::new(steps));

        assert_eq!(at(steps), Verdict::Holds, "{condition} {steps}");
        assert_eq!(
            at(steps - 1),
            Verdict::Refused(Refusal::BudgetExceeded),
            "{condition} {steps}"
        );
    }
}

#[test]
fn test_a_stopped_budget_refuses_at_its_next_step() {
    let condition = json!({"path": "request.headers.x-a", "op": "eq", "value": "a"});
    let stop = Arc::new(AtomicBool::new(false));
    let evaluate = || {
        let mut budget = Budget::default().stopped_by(Arc::clone(&stop));
        evaluate_on_header(condition.clone(), "a", &mut budget)
    };

    assert_eq!(evaluate(), Verdict::Holds);
    stop.store(true, Ordering::Relaxed);
    assert_eq!(evaluate(), Verdict::Refused(Refusal::BudgetExceeded));
}

#[test]
fn test_matches_a_hostile_pattern_in_one_scan() {
    // A backtracking matcher tries every split of the run of `a` before failing on the `c`.
    let pattern = format!("{}*c*b", "*a".repeat(1_000));
    let condition = json!({"path": "request.headers.x-a", "op": "matches", "value": pattern});
    let value = format!("{}b", "a".repeat(100_000));

    let mut budget = Budget::default();
    let started = std::time::Instant::now();
    assert_eq!(
        evaluate_on_header(condition, &value, &mut budget),
        Verdict::DoesNotHold
    );
    assert!(
        started.elapsed().as_millis() < 100,
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn test_charges_a_long_value_by_its_length() {
    let condition = json!({"path": "request.headers.x-a", "op": "matches", "value": "*"});
    let value = "a".repeat(6_400);

    // 102 steps: refused at a limit of 101, held at 102.
    assert_eq!(
        evaluate_on_header(condition.clone(), &value, &mut Budget::new(101)),
        Verdict::Refused(Refusal::BudgetExceeded)
    );
    assert_eq!(
        evaluate_on_header(condition, &value, &mut Budget::new(102)),
        Verdict::Holds
    );
}

// As a plan writes a condition, its path a list.
fn evaluate_on_call(condition: Value, body: &BodyRead, budget: &mut Budget) -> Verdict {
    let request = observed(
        "mcp.example",
        "POST",
        "/finretail-production-mcp/mcp",
        body.clone(),
        0,
    );
    let predicate: Predicate = serde_json::from_value(condition).unwrap();
    let environment = Environment::of(&request, None);
    evaluate_condition(
        &compile_condition(&Condition::Valid(predicate), ConditionUse::CapturePlan),
        &environment,
        budget,
    )
    .verdict
}

fn condition(path: Value, op: &str, value: Value) -> Value {
    json!({"path": path, "op": op, "value": value})
}

#[test]
fn test_charges_an_index_by_the_items_it_passes() {
    let body = keep_body(br#"{"items": [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]}"#);
    let at = |index: usize| {
        condition(
            json!(["request", "body", "items", index]),
            "existing",
            json!(null),
        )
    };
    assert_eq!(
        evaluate_on_call(at(8), &body, &mut Budget::new(11)),
        Verdict::Holds
    );
    assert_eq!(
        evaluate_on_call(at(9), &body, &mut Budget::new(11)),
        Verdict::Refused(Refusal::BudgetExceeded)
    );
    // Charged only what it passes: past the end, and past any number a step can hold.
    assert_eq!(
        evaluate_on_call(at(200_000), &body, &mut Budget::new(12)),
        Verdict::DoesNotHold
    );
    assert_eq!(
        evaluate_on_call(at(usize::MAX), &body, &mut Budget::new(12)),
        Verdict::DoesNotHold
    );
}

// Every member read, as a key repeated past the first must be.
#[test]
fn test_charges_a_key_by_the_members_it_reads() {
    let body = keep_body(
        br#"{"a": 0, "b": 1, "c": 2, "d": 3, "e": 4, "f": 5, "g": 6, "h": 7, "i": 8, "j": 9}"#,
    );
    let first = || condition(json!(["request", "body", "a"]), "existing", json!(null));
    assert_eq!(
        evaluate_on_call(first(), &body, &mut Budget::new(11)),
        Verdict::Holds
    );
    assert_eq!(
        evaluate_on_call(first(), &body, &mut Budget::new(10)),
        Verdict::Refused(Refusal::BudgetExceeded)
    );
}

// A policy reads a request's header lines, every value split on commas, a byte not UTF-8 replaced.
#[test]
fn test_reads_a_policys_headers_from_an_observed_request() {
    let header = vec![(b"X-A".to_vec(), b"1, a\xff".to_vec())];
    let (authority, method, path) = (b"mcp.example".to_vec(), b"POST".to_vec(), b"/mcp".to_vec());
    let request = ObservedRequest::new(authority, method, path, header, keep_body(b"{}"), 2);
    let condition = json!({"path": "request.headers.x-a", "op": "contains", "value": "a\u{fffd}"});
    let compiled = compile_condition(
        &serde_json::from_value(condition).unwrap(),
        ConditionUse::GatewayPolicy,
    );
    let environment = Environment::of(&request, None);
    assert_eq!(
        evaluate_condition(&compiled, &environment, &mut Budget::default()).verdict,
        Verdict::Holds
    );
}
