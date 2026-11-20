use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rail_model::body::{BodyBuffer, BodyRead, ByteBudget, Unparsed, Unread};
use rail_model::condition::{
    BodyUnread, Budget, Condition, ConditionUse, Environment, Headers, Predicate, Refusal, Verdict,
    compile_condition, evaluate_condition,
};
use rail_model::endpoint::Endpoint;
use rail_model::request_line::RequestLine;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

const VECTORS: &str = include_str!("../../../../vectors/judge_condition_conformance_cases.json");

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
    let environment = Environment::for_judging(case.ticket.as_ref(), &endpoint, &headers);
    evaluate_condition(
        &compile_condition(&condition, ConditionUse::Policy),
        &environment,
        &mut Budget::default(),
    )
    .verdict
}

fn get_outcome_name(outcome: &Verdict) -> &'static str {
    match outcome {
        Verdict::Holds => "holds",
        Verdict::DoesNotHold => "does_not_hold",
        Verdict::Ambiguous => "ambiguous",
        Verdict::BodyUnread(_) => "body_unread",
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
    let environment = Environment::for_judging(None, &Endpoint::Keyless, &headers);
    evaluate_condition(
        &compile_condition(&condition, ConditionUse::Policy),
        &environment,
        budget,
    )
    .verdict
}

fn evaluate_on_endpoint(condition: Value, endpoint: &Endpoint) -> Verdict {
    let condition: Condition = serde_json::from_value(condition).unwrap();
    let headers = Headers::from_lines([("x-a", "a")]);
    let environment = Environment::for_judging(None, endpoint, &headers);
    evaluate_condition(
        &compile_condition(&condition, ConditionUse::Policy),
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
    let environment = Environment::for_judging(Some(&claims), &Endpoint::Keyless, &headers);
    evaluate_condition(
        &compile_condition(&condition, ConditionUse::Policy),
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

fn keep_body(body: &[u8]) -> BodyRead {
    let mut buffer = BodyBuffer::new(usize::MAX, &ByteBudget::new(usize::MAX));
    buffer.append(body);
    BodyRead::Whole(Arc::new(buffer.finish().unwrap()))
}

// As a plan writes a condition, its path a list.
fn evaluate_on_call(condition: Value, body: &BodyRead, budget: &mut Budget) -> Verdict {
    let request_line = RequestLine::read(b"mcp.example", b"POST", b"/finretail-production-mcp/mcp");
    let predicate: Predicate = serde_json::from_value(condition).unwrap();
    let environment = Environment::for_capture(&request_line, body);
    evaluate_condition(
        &compile_condition(&Condition::Valid(predicate), ConditionUse::PlanMatch),
        &environment,
        budget,
    )
    .verdict
}

fn condition(path: Value, op: &str, value: Value) -> Value {
    json!({"path": path, "op": op, "value": value})
}

#[test]
fn test_reads_a_calls_request_line_and_mcp_message() {
    let body = keep_body(br#"{"method": "tools/call", "params": {"name": "a"}}"#);
    for (path, value) in [
        (json!(["request", "host"]), "mcp.example"),
        (json!(["request", "method"]), "POST"),
        (json!(["request", "path"]), "/finretail-production-mcp/mcp"),
        (json!(["request", "redacted_path"]), "/[redacted]/mcp"),
        (json!(["request", "mcp", "method"]), "tools/call"),
        (json!(["request", "mcp", "tool"]), "a"),
    ] {
        let held = evaluate_on_call(
            condition(path.clone(), "eq", json!(value)),
            &body,
            &mut Budget::default(),
        );
        assert_eq!(held, Verdict::Holds, "{path}");
    }
}

#[test]
fn test_reads_a_body_path_and_tells_what_cant_be_told() {
    let body = keep_body(
        br#"{"esc": "pay\u0072oll", "items": ["a", "b"], "big": 1e400, "dup": 1, "Dup": 2,
            "bad": "\ud800", "mixed": ["a", "\ud800"]}"#,
    );
    let ambiguous = Verdict::Ambiguous;
    for (path, op, value, verdict) in [
        (
            json!(["request", "body", "esc"]),
            "eq",
            json!("payroll"),
            Verdict::Holds,
        ),
        (
            json!(["request", "body", "items", 1]),
            "eq",
            json!("b"),
            Verdict::Holds,
        ),
        (
            json!(["request", "body", "items"]),
            "contains",
            json!("a"),
            Verdict::Holds,
        ),
        (
            json!(["request", "body", "items", "1"]),
            "existing",
            json!(null),
            Verdict::DoesNotHold,
        ),
        (
            json!(["request", "body", "missing"]),
            "not_existing",
            json!(null),
            Verdict::Holds,
        ),
        (
            json!(["request", "body"]),
            "existing",
            json!(null),
            Verdict::Holds,
        ),
        // Past a double, saturated.
        (
            json!(["request", "body", "big"]),
            "gt",
            json!(1e300),
            Verdict::Holds,
        ),
        // Whatever the op, absent included: readers differ on what's there.
        (json!(["request", "body", "dup"]), "eq", json!(1), ambiguous),
        (
            json!(["request", "body", "dup"]),
            "not_existing",
            json!(null),
            ambiguous,
        ),
        (
            json!(["request", "body", "bad"]),
            "existing",
            json!(null),
            ambiguous,
        ),
        (
            json!(["request", "body", "mixed"]),
            "contains",
            json!("a"),
            ambiguous,
        ),
    ] {
        let verdict_read = evaluate_on_call(
            condition(path.clone(), op, value),
            &body,
            &mut Budget::default(),
        );
        assert_eq!(verdict_read, verdict, "{path} {op}");
    }
    let empty = keep_body(b"");
    let held = evaluate_on_call(
        condition(json!(["request", "body", "a"]), "not_existing", json!(null)),
        &empty,
        &mut Budget::default(),
    );
    assert_eq!(held, Verdict::Holds, "an empty body holds nothing");
}

#[test]
fn test_tells_why_a_body_value_isnt_read() {
    let tool = || {
        condition(
            json!(["request", "mcp", "tool"]),
            "not_existing",
            json!(null),
        )
    };
    let key = || {
        condition(
            json!(["request", "body", "method"]),
            "not_existing",
            json!(null),
        )
    };
    for (body, verdict) in [
        (
            BodyRead::Unread(Unread::NotSent),
            Verdict::BodyUnread(BodyUnread::Unread(Unread::NotSent)),
        ),
        (
            keep_body(b"not json"),
            Verdict::BodyUnread(BodyUnread::Unparsed(Unparsed::Malformed)),
        ),
    ] {
        assert_eq!(
            evaluate_on_call(tool(), &body, &mut Budget::default()),
            verdict
        );
        assert_eq!(
            evaluate_on_call(key(), &body, &mut Budget::default()),
            verdict
        );
    }
    // A batch's items are read as a body, but name no one MCP message.
    let batch = keep_body(br#"[{"method": "tools/call", "params": {"name": "a"}}]"#);
    assert_eq!(
        evaluate_on_call(tool(), &batch, &mut Budget::default()),
        Verdict::BodyUnread(BodyUnread::Batch)
    );
    let first = condition(
        json!(["request", "body", 0, "method"]),
        "eq",
        json!("tools/call"),
    );
    assert_eq!(
        evaluate_on_call(first, &batch, &mut Budget::default()),
        Verdict::Holds
    );
    let repeated = keep_body(br#"{"method": "tools/call", "params": {"name": "a", "Name": "b"}}"#);
    assert_eq!(
        evaluate_on_call(tool(), &repeated, &mut Budget::default()),
        Verdict::Ambiguous
    );
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

#[test]
fn test_refuses_a_condition_with_its_cause() {
    // In an environment that fills each path, so only compiling for a policy refuses them.
    let request_line = RequestLine::read(b"mcp.example", b"POST", b"/mcp");
    let body = keep_body(b"{}");
    let capture = Environment::for_capture(&request_line, &body);
    for (condition, refusal) in [
        (
            json!({"path": "ticket.a", "op": "newer_op"}),
            Refusal::UnknownOp,
        ),
        (
            json!({"path": "ticket.a", "op": "lt", "value": "x"}),
            Refusal::Invalid,
        ),
        (
            json!({"path": "request.host", "op": "existing"}),
            Refusal::NotForUse,
        ),
        (
            json!({"path": "request.mcp.tool", "op": "existing"}),
            Refusal::NotForUse,
        ),
        (
            json!({"path": "request.body.a", "op": "not_existing"}),
            Refusal::NotForUse,
        ),
    ] {
        let read: Condition = serde_json::from_value(condition.clone()).unwrap();
        let compiled = compile_condition(&read, ConditionUse::Policy);
        let verdict = evaluate_condition(&compiled, &capture, &mut Budget::default()).verdict;
        assert_eq!(verdict, Verdict::Refused(refusal), "{condition}");
    }
    for path in [
        json!(["ticket", "a"]),
        json!(["request", "endpoint_key"]),
        json!(["request", "headers", "a"]),
        json!(["request"]),
        json!(["query", "a", 0]),
        json!(["path", 0]),
    ] {
        let held = evaluate_on_call(
            condition(path.clone(), "not_existing", json!(null)),
            &body,
            &mut Budget::default(),
        );
        assert_eq!(held, Verdict::Refused(Refusal::NotForUse), "{path}");
    }
}

// Compiled for one use, evaluated where another's environment fills other sources.
#[test]
fn test_refuses_a_source_its_environment_doesnt_fill() {
    let not_filled = Verdict::Refused(Refusal::NotForUse);
    let request_line = RequestLine::read(b"mcp.example", b"POST", b"/mcp");
    let body = keep_body(b"{}");
    let capture = Environment::for_capture(&request_line, &body);
    for path in [
        "request.endpoint_key",
        "request.headers",
        "request.headers.a",
        "ticket.a",
    ] {
        let condition: Condition =
            serde_json::from_value(json!({"path": path, "op": "not_existing"})).unwrap();
        let compiled = compile_condition(&condition, ConditionUse::Policy);
        let verdict = evaluate_condition(&compiled, &capture, &mut Budget::default()).verdict;
        assert_eq!(verdict, not_filled, "{path}");
    }
    let headers = Headers::default();
    let judging = Environment::for_judging(None, &Endpoint::Keyless, &headers);
    for path in [json!(["request", "body", "a"]), json!(["request", "host"])] {
        let predicate: Predicate =
            serde_json::from_value(json!({"path": path, "op": "not_existing"})).unwrap();
        let compiled = compile_condition(&Condition::Valid(predicate), ConditionUse::PlanMatch);
        let verdict = evaluate_condition(&compiled, &judging, &mut Budget::default()).verdict;
        assert_eq!(verdict, not_filled, "{path}");
    }
}
