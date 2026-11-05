use rail_policy::{
    Budget, Condition, Endpoint, Environment, Headers, Refusal, Verdict, compile_condition,
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

fn evaluate_case(case: &Case) -> Verdict {
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
    .verdict
}

fn get_outcome_name(outcome: &Verdict) -> &'static str {
    match outcome {
        Verdict::Holds => "holds",
        Verdict::DoesNotHold => "does_not_hold",
        Verdict::Refused(_) => "refused",
    }
}

#[test]
fn test_vectors() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());

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

fn evaluate_on_header(condition: Value, value: &str, budget: &mut Budget) -> Verdict {
    let condition: Condition = serde_json::from_value(condition).unwrap();
    let headers = Headers::from_lines([("x-a", value)]);
    let environment = Environment {
        ticket: None,
        endpoint: &Endpoint::Keyless,
        headers: &headers,
    };
    evaluate_condition(&compile_condition(&condition), &environment, budget).verdict
}

fn evaluate_on_endpoint(condition: Value, endpoint: &Endpoint) -> Verdict {
    let condition: Condition = serde_json::from_value(condition).unwrap();
    let headers = Headers::from_lines([("x-a", "a")]);
    let environment = Environment {
        ticket: None,
        endpoint,
        headers: &headers,
    };
    evaluate_condition(
        &compile_condition(&condition),
        &environment,
        &mut Budget::default(),
    )
    .verdict
}

#[test]
fn test_reads_the_endpoint_key_of_a_keyless_or_unrecognised_call_as_absent() {
    let key = |op, value| json!({"path": "request.endpoint_key", "op": op, "value": value});
    let resolved = Endpoint::Resolved("/mcp#tools/call#t".parse().unwrap());
    assert_eq!(
        evaluate_on_endpoint(key("existing", json!(null)), &resolved),
        Verdict::Holds
    );
    for endpoint in [Endpoint::Keyless, Endpoint::Unrecognised] {
        for (op, value, verdict) in [
            ("existing", json!(null), Verdict::DoesNotHold),
            ("not_existing", json!(null), Verdict::Holds),
            ("ne", json!("x"), Verdict::DoesNotHold),
        ] {
            assert_eq!(
                evaluate_on_endpoint(key(op, value), &endpoint),
                verdict,
                "{endpoint:?} {op}"
            );
        }
    }
}

#[test]
fn test_reads_the_request_and_its_headers_as_objects() {
    for path in ["request", "request.headers"] {
        for (op, value, verdict) in [
            ("existing", json!(null), Verdict::Holds),
            ("eq", json!("x"), Verdict::DoesNotHold),
        ] {
            assert_eq!(
                evaluate_on_endpoint(
                    json!({"path": path, "op": op, "value": value}),
                    &Endpoint::Keyless
                ),
                verdict,
                "{path} {op}"
            );
        }
    }
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
    let mut budget = Budget::default();

    assert_eq!(
        evaluate_on_header(condition, &"a".repeat(6_400), &mut budget),
        Verdict::Holds
    );
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

fn evaluate_on_claims(condition: Value, claims: Value) -> Verdict {
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
    .verdict
}

#[test]
fn test_matches_interior_literals_in_order_without_reuse() {
    let read = |pattern: &str, value: &str| {
        let condition = json!({"path": "request.headers.x-a", "op": "matches", "value": pattern});
        evaluate_on_header(condition, value, &mut Budget::default()) == Verdict::Holds
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
        Verdict::DoesNotHold
    );

    let condition = json!({"path": "ticket.posture.overall.score", "op": "in", "value": [true]});
    assert_eq!(
        evaluate_on_claims(condition, json!({"posture": {"overall": {"score": 1}}})),
        Verdict::DoesNotHold
    );
}

#[test]
fn test_reads_one_header_value_as_a_string() {
    let condition = json!({"path": "request.headers.x-a", "op": "contains", "value": "acme"});
    assert_eq!(
        evaluate_on_header(condition, "acme", &mut Budget::default()),
        Verdict::DoesNotHold
    );
}

#[test]
fn test_reads_what_a_ticket_leaves_out_as_absent() {
    let claims = json!({
        "posture": {"categories": {"identity": {"score": null, "capped": true}}},
        "alignment": {"drifted": []},
    });
    let read = |path: &str| {
        let condition = json!({"path": path, "op": "existing"});
        evaluate_on_claims(condition, claims.clone()) == Verdict::Holds
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
        evaluate_on_claims(condition, json!({"skills": skills})) == Verdict::Holds
    };
    assert!(!read("eq", json!("acme"), json!(["acme", null])));
    assert!(read("ne", json!("acme"), json!(["acme", null])));
    assert!(read("ne", json!("acme"), json!(["acme", {}])));
    assert!(!read("in", json!(["acme"]), json!([null, true])));
    assert!(!read("matches", json!("*"), json!([true, null, 1])));
    assert!(read("matches", json!("*"), json!([true, "a"])));
}
