use rail_policy::{
    Budget, Condition, Endpoint, Environment, Headers, Refusal, ResolvedKey, compile_condition,
    evaluate_condition,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

const VECTORS: &str = include_str!("../../../vectors/judge_condition_conformance_cases.json");

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    // Raw, since some operands are past what a `Value` holds.
    condition: Box<RawValue>,
    ticket: Option<Map<String, Value>>,
    endpoint_key: Option<String>,
    headers: Vec<(String, String)>,
    expect: String,
}

fn evaluate_case(case: &Case) -> Result<bool, Refusal> {
    let condition: Condition = serde_json::from_str(case.condition.get()).unwrap();
    let endpoint = match &case.endpoint_key {
        Some(key) => Endpoint::Resolved(key.parse().unwrap()),
        None => Endpoint::Keyless,
    };
    let headers = Headers::from_lines(case.headers.iter().map(|(n, v)| (n, v)));
    let environment = Environment {
        ticket: case.ticket.as_ref(),
        endpoint: &endpoint,
        headers: &headers,
    };
    evaluate_condition(
        &compile_condition(&condition),
        &environment,
        &mut Budget::default(),
    )
}

fn get_outcome_name(outcome: &Result<bool, Refusal>) -> &'static str {
    match outcome {
        Ok(true) => "holds",
        Ok(false) => "does_not_hold",
        Err(_) => "refused",
    }
}

#[test]
fn test_vectors() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();

    let failures: Vec<String> = vectors
        .cases
        .iter()
        .filter_map(|case| {
            let outcome = evaluate_case(case);
            let name = get_outcome_name(&outcome);
            (name != case.expect)
                .then(|| format!("{}: want {}, got {outcome:?}", case.name, case.expect))
        })
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

fn evaluate_on_header(condition: Value, value: &str, budget: &mut Budget) -> Result<bool, Refusal> {
    let condition: Condition = serde_json::from_value(condition).unwrap();
    let headers = Headers::from_lines([("x-a", value)]);
    let environment = Environment {
        ticket: None,
        endpoint: &Endpoint::Keyless,
        headers: &headers,
    };
    evaluate_condition(&compile_condition(&condition), &environment, budget)
}

#[test]
fn test_refuses_past_the_step_limit_and_not_before() {
    let condition = json!({"path": "request.headers.x-a", "op": "eq", "value": "a"});

    // One step for the condition, and one for each of the three values.
    assert_eq!(
        evaluate_on_header(condition.clone(), "a,a,a", &mut Budget::new(4)),
        Ok(true)
    );
    assert_eq!(
        evaluate_on_header(condition, "a,a,a", &mut Budget::new(3)),
        Err(Refusal::BudgetExceeded)
    );
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
        Ok(false)
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
    let mut budget = Budget::default();

    evaluate_on_header(condition, &"a".repeat(6_400), &mut budget).unwrap();
    assert_eq!(budget.steps(), 102);
}

#[test]
fn test_folds_header_lines() {
    let headers = Headers::from_lines([("X-A", "1, 2"), ("x-a", "3"), ("x-b", "a,,b ")]);
    let expected = Headers::from_lines([
        ("x-a", "1"),
        ("x-a", "2"),
        ("x-a", "3"),
        ("x-b", "a"),
        ("x-b", ""),
        ("x-b", "b"),
    ]);
    assert_eq!(headers, expected);
}

#[test]
fn test_reads_a_resolved_key() {
    let key: ResolvedKey = "/mcp#tools/call#a#b".parse().unwrap();
    assert_eq!(
        (key.path(), key.method(), key.call()),
        ("/mcp", "tools/call", "a#b")
    );
    assert_eq!(key.as_str(), "/mcp#tools/call#a#b");

    for text in [
        "/mcp#tools/call",
        "/mcp#tools/call#",
        "#tools/call#a",
        "/mcp##a",
        "",
    ] {
        assert!(text.parse::<ResolvedKey>().is_err(), "{text:?}");
    }
}

fn evaluate_on_claims(condition: Value, claims: Value) -> Result<bool, Refusal> {
    let condition: Condition = serde_json::from_value(condition).unwrap();
    let Value::Object(claims) = claims else {
        panic!();
    };
    let headers = Headers::default();
    let environment = Environment {
        ticket: Some(&claims),
        endpoint: &Endpoint::Keyless,
        headers: &headers,
    };
    evaluate_condition(
        &compile_condition(&condition),
        &environment,
        &mut Budget::default(),
    )
}

fn get_claims(score: f64, categories: Value) -> Value {
    json!({
        "id": "10000000-0000-4000-8000-000000000001",
        "agent_id": "20000000-0000-4000-8000-000000000002",
        "sandbox_id": "sandbox-1",
        "environment_fingerprint": "fingerprint-1",
        "skills": [],
        "posture": {"overall": {"score": score, "level": "low"}, "categories": categories},
        "alignment": {"drifted": []},
    })
}

#[test]
fn test_matches_interior_literals_in_order_without_reuse() {
    let read = |pattern: &str, value: &str| {
        let condition = json!({"path": "request.headers.x-a", "op": "matches", "value": pattern});
        evaluate_on_header(condition, value, &mut Budget::default()).unwrap()
    };
    assert!(read("x*a*b*y", "xaby"));
    assert!(!read("x*b*a*y", "xaby"));
    assert!(!read("x*ab*ab*y", "xaby"));
    assert!(read("x*ab*ab*y", "xababy"));
}

#[test]
fn test_never_reads_a_boolean_operand_as_a_member() {
    let condition = json!({"path": "request.headers.x-a", "op": "in", "value": [true]});
    assert_eq!(
        evaluate_on_header(condition, "true", &mut Budget::default()),
        Ok(false)
    );

    let condition = json!({"path": "ticket.posture.overall.score", "op": "in", "value": [true]});
    assert_eq!(
        evaluate_on_claims(condition, get_claims(1.0, json!({}))),
        Ok(false)
    );
}

#[test]
fn test_reads_one_header_value_as_a_string() {
    let condition = json!({"path": "request.headers.x-a", "op": "contains", "value": "acme"});
    assert_eq!(
        evaluate_on_header(condition, "acme", &mut Budget::default()),
        Ok(false)
    );
}

#[test]
fn test_reads_what_a_ticket_leaves_out_as_absent() {
    let claims = get_claims(1.0, json!({"identity": {"score": null, "capped": true}}));
    let read = |path: &str| {
        let condition = json!({"path": path, "op": "existing"});
        evaluate_on_claims(condition, claims.clone()).unwrap()
    };
    assert!(!read("ticket.alignment.baseline"));
    assert!(!read("ticket.posture.categories.network"));
    assert!(!read("ticket.posture.categories.network.capped"));
    assert!(read("ticket.posture.categories.identity"));
    assert!(read("ticket.posture.categories.identity.capped"));
    assert!(!read("ticket.posture.categories.identity.score"));
}

#[test]
fn test_reads_a_list_item_that_is_not_a_scalar_as_matching_nothing() {
    let read = |op: &str, value: Value, skills: Value| {
        let condition = json!({"path": "ticket.skills", "op": op, "value": value});
        evaluate_on_claims(condition, json!({"exp": 60, "skills": skills})).unwrap()
    };
    assert!(!read("eq", json!("acme"), json!(["acme", null])));
    assert!(read("ne", json!("acme"), json!(["acme", null])));
    assert!(read("ne", json!("acme"), json!(["acme", {}])));
    assert!(!read("in", json!(["acme"]), json!([null, true])));
    assert!(!read("matches", json!("*"), json!([true, null, 1])));
    assert!(read("matches", json!("*"), json!([true, "a"])));
}
