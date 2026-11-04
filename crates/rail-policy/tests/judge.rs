use rail_policy::{
    Budget, Decision, Endpoint, EnforcementMode, Environment, FallbackDenial, Headers, JudgingMode,
    Lookup, Outcome, Refusal, Trace, compile_bundle, judge, judge_traced, parse_bundle,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const VECTORS: &str = include_str!("../../../vectors/judge_walk_conformance_cases.json");

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
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

/// Each step as `(policy's last two digits, what was resolved, result)`.
#[derive(Default)]
struct Recorder(Vec<(String, Option<String>, Result<bool, Refusal>)>);

impl Trace for Recorder {
    fn visit(
        &mut self,
        policy: Uuid,
        resolved: Option<Lookup<'_>>,
        result: &Result<bool, Refusal>,
    ) {
        let id = policy.to_string();
        let resolved = resolved.map(|lookup| format!("{lookup:?}"));
        self.0
            .push((id[id.len() - 2..].to_string(), resolved, *result));
    }
}

fn traced_policy(n: u32, condition: Value, action: &str) -> Value {
    json!({
        "id": format!("5c8f1e42-0000-4000-8000-0000000000{n:02}"),
        "name": format!("P-{n}"),
        "priority": n,
        "condition": condition,
        "action": action,
        "enabled": true,
    })
}

fn trace_bundle(policies: Value, bindings: Value, limit: u64) -> (Outcome, Recorder) {
    let bundle = json!({
        "schema_version": "2.0",
        "content_hash": "trace",
        "enforcement": {"mode": "enforce"},
        "policies": policies,
        "bindings": bindings,
        "binding_fallback": "pass",
        "ticket_fallback": "pass",
    });
    let claims = json!({"admin": true, "score": 10})
        .as_object()
        .unwrap()
        .clone();
    let headers = Headers::from_lines([("x-a", "b, c")]);
    let endpoint = Endpoint::Resolved("/mcp#tools/call#t".parse().unwrap());
    let environment = Environment {
        ticket: Some(&claims),
        endpoint: &endpoint,
        headers: &headers,
    };
    let mut recorder = Recorder::default();
    let outcome = judge_traced(
        &compile_bundle(&parse_bundle(&bundle.to_string()).unwrap()),
        &environment,
        JudgingMode::Enforce,
        &mut Budget::new(limit),
        &mut recorder,
    );
    (outcome, recorder)
}

#[test]
fn test_traces_each_policy_evaluated_until_one_decides() {
    let policies = json!([
        traced_policy(
            1,
            json!({"path": "ticket.admin", "op": "existing"}),
            "alert"
        ),
        traced_policy(
            2,
            json!({"path": "ticket.missing", "op": "existing"}),
            "block"
        ),
        traced_policy(
            3,
            json!({"path": "request.headers.x-a", "op": "contains", "value": "c"}),
            "block"
        ),
        traced_policy(
            4,
            json!({"path": "ticket.score", "op": "lt", "value": 40}),
            "block"
        ),
    ]);

    let (outcome, recorder) = trace_bundle(policies, json!([]), 100);

    assert_eq!(
        outcome.chain.unwrap().decision,
        Decision::Denied("5c8f1e42-0000-4000-8000-000000000003".parse().unwrap())
    );
    assert_eq!(
        recorder.0,
        [
            ("01".into(), Some("Bool(true)".into()), Ok(true)),
            ("02".into(), Some("Absent".into()), Ok(false)),
            (
                "03".into(),
                Some(r#"List([String("b"), String("c")])"#.into()),
                Ok(true)
            ),
        ]
    );
}

#[test]
fn test_traces_a_refusal_with_nothing_resolved() {
    let policies = json!([
        traced_policy(
            1,
            json!({"path": "ticket.score", "op": "lt", "value": 40}),
            "alert"
        ),
        traced_policy(
            2,
            json!({"path": "ticket.score", "op": "lt", "value": "x"}),
            "block"
        ),
        traced_policy(
            3,
            json!({"path": "ticket.score", "op": "lt", "value": 40}),
            "block"
        ),
    ]);

    let (_, recorder) = trace_bundle(policies.clone(), json!([]), 100);
    assert_eq!(
        recorder.0,
        [
            ("01".into(), Some("Number(10.0)".into()), Ok(true)),
            ("02".into(), None, Err(Refusal::Uninterpretable)),
        ]
    );

    // The first step is past the limit, so no path is resolved.
    let (_, recorder) = trace_bundle(policies, json!([]), 0);
    assert_eq!(
        recorder.0,
        [("01".into(), None, Err(Refusal::BudgetExceeded))]
    );
}

#[test]
fn test_traces_only_the_policies_a_binding_allows() {
    let policies = json!([
        traced_policy(
            1,
            json!({"path": "ticket.score", "op": "gt", "value": 40}),
            "block"
        ),
        traced_policy(
            2,
            json!({"path": "ticket.score", "op": "gt", "value": 40}),
            "block"
        ),
    ]);
    let bindings = json!([{
        "endpoint_key": "slug#/mcp#tools/call#t",
        "mode": "gated",
        "policy_ids": ["5c8f1e42-0000-4000-8000-000000000002"],
    }]);

    let (_, recorder) = trace_bundle(policies, bindings, 100);

    assert_eq!(
        recorder.0,
        [("02".into(), Some("Number(10.0)".into()), Ok(false))]
    );
}
