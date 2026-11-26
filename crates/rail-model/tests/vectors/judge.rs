use rail_model::condition::{Budget, Environment};
use rail_model::endpoint::Endpoint;
use rail_model::policy::{
    Decision, EnforcementMode, FallbackDenial, Outcome, compile_bundle, judge, parse_bundle,
};
use rail_model::request::Headers;
use serde::Deserialize;
use serde_json::{Map, Value, json};

const VECTORS: &str = include_str!("../../../../vectors/policy/judge.json");

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    bundle: Value,
    mode: EnforcementMode,
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
        .fallback_denials
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
    let environment = Environment::for_judging(case.ticket.as_ref(), &endpoint, &headers);
    judge(
        &compile_bundle(&bundle),
        &environment,
        case.mode.judging_mode().unwrap(),
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
                .then(|| format!("{}:\n  want {}\n  got  {got}", case.id, case.expect))
        })
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
