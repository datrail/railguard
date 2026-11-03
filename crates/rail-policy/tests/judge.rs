use rail_policy::{
    Budget, Decision, Endpoint, EnforcementMode, Environment, FallbackDenial, Headers, JudgingMode,
    Outcome, Refusal, compile_bundle, judge, parse_bundle,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const VECTORS: &str = include_str!("../../../vectors/judge_walk_conformance_cases.json");

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    bundle: Value,
    mode: JudgingMode,
    endpoint: Value,
    ticket: Option<Map<String, Value>>,
    headers: Vec<(String, String)>,
    expect: Value,
}

fn read_endpoint(endpoint: &Value) -> Endpoint {
    match endpoint {
        Value::String(kind) if kind == "keyless" => Endpoint::Keyless,
        Value::String(kind) if kind == "unrecognised" => Endpoint::Unrecognised,
        _ => Endpoint::Resolved(endpoint["resolved"].as_str().unwrap().parse().unwrap()),
    }
}

/// The outcome in the vectors' own shape, so a failure shows both sides alike.
fn format_outcome(outcome: &Outcome) -> Value {
    let fallbacks: Vec<&str> = outcome
        .fallbacks
        .iter()
        .map(|fallback| match fallback {
            FallbackDenial::Ticket => "ticket",
            FallbackDenial::Binding => "binding",
        })
        .collect();
    let chain = outcome.chain.as_ref().map(|chain| {
        let decision = match &chain.decision {
            Decision::Allowed => json!("allowed"),
            Decision::Denied(id) => json!({"denied": id}),
            Decision::Refused { policy, .. } => json!({"refused": policy}),
        };
        json!({"decision": decision, "alerts": chain.alerts})
    });
    json!({"fallbacks": fallbacks, "chain": chain})
}

fn judge_case(case: &Case) -> Outcome {
    let bundle = parse_bundle(&case.bundle.to_string()).unwrap();
    let endpoint = read_endpoint(&case.endpoint);
    let headers = Headers::from_lines(case.headers.iter().map(|(n, v)| (n, v)));
    let environment = Environment {
        ticket: case.ticket.as_ref(),
        endpoint: &endpoint,
        headers: &headers,
    };
    judge(
        &compile_bundle(&bundle),
        &environment,
        case.mode,
        &mut Budget::default(),
    )
}

#[test]
fn test_vectors() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());

    let failures: Vec<String> = vectors
        .cases
        .iter()
        .filter_map(|case| {
            let got = format_outcome(&judge_case(case));
            (got != case.expect)
                .then(|| format!("{}:\n  want {}\n  got  {got}", case.name, case.expect))
        })
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn test_shares_one_budget_across_the_walk() {
    let policy = |n: u32| {
        json!({
            "id": format!("5c8f1e42-0000-4000-8000-0000000000{n:02}"),
            "name": format!("P-{n}"),
            "priority": n,
            "condition": {"path": "request.headers.x-a", "op": "eq", "value": "b"},
            "action": "block",
            "enabled": true,
        })
    };
    let bundle = json!({
        "schema_version": "2.0",
        "content_hash": "budget",
        "enforcement": {"mode": "enforce"},
        "policies": [policy(1), policy(2), policy(3)],
        "bindings": [],
        "binding_fallback": "pass",
        "ticket_fallback": "pass",
    });
    let judge_with = |limit: u64| {
        let headers = Headers::from_lines([("x-a", "a")]);
        let environment = Environment {
            ticket: None,
            endpoint: &Endpoint::Keyless,
            headers: &headers,
        };
        let judge_bundle = compile_bundle(&parse_bundle(&bundle.to_string()).unwrap());
        judge(
            &judge_bundle,
            &environment,
            JudgingMode::Enforce,
            &mut Budget::new(limit),
        )
        .chain
        .unwrap()
        .decision
    };

    // One step per condition: three policies take three.
    assert_eq!(judge_with(3), Decision::Allowed);
    assert_eq!(
        judge_with(2),
        Decision::Refused {
            policy: "5c8f1e42-0000-4000-8000-000000000003".parse().unwrap(),
            refusal: Refusal::BudgetExceeded,
        }
    );
}

#[test]
fn test_judges_nothing_at_mode_none() {
    assert_eq!(EnforcementMode::None.judging_mode(), None);
    assert_eq!(
        EnforcementMode::Observe.judging_mode(),
        Some(JudgingMode::Observe)
    );
    assert_eq!(
        EnforcementMode::Enforce.judging_mode(),
        Some(JudgingMode::Enforce)
    );
}
