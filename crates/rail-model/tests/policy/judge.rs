use std::sync::Arc;

use rail_model::body::{BodyBuffer, BodyRead, ByteBudget, Unread};
use rail_model::condition::{Budget, Environment, Evaluation, Refusal, Verdict};
use rail_model::endpoint::Endpoint;
use rail_model::policy::{
    Decision, EnforcementMode, FallbackDenial, JudgingMode, Outcome, Tracer, compile_bundle, judge,
    judge_traced, parse_bundle,
};
use rail_model::request::{Headers, ObservedRequest};
use serde_json::{Value, json};
use uuid::Uuid;

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
        let environment = Environment::for_judging(None, &Endpoint::Keyless, &headers);
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
struct Recorder(Vec<(String, Option<String>, Verdict)>);

impl Tracer for Recorder {
    fn record(&mut self, policy: Uuid, evaluation: &Evaluation) {
        let id = policy.to_string();
        let resolved = evaluation
            .resolved
            .as_ref()
            .map(|lookup| format!("{lookup:?}"));
        self.0
            .push((id[id.len() - 2..].to_string(), resolved, evaluation.verdict));
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
    let environment = Environment::for_judging(Some(&claims), &endpoint, &headers);
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
            ("01".into(), Some("Bool(true)".into()), Verdict::Holds),
            ("02".into(), Some("Absent".into()), Verdict::DoesNotHold),
            (
                "03".into(),
                Some(r#"List(Values([String("b"), String("c")]))"#.into()),
                Verdict::Holds
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
            ("01".into(), Some("Number(10.0)".into()), Verdict::Holds),
            ("02".into(), None, Verdict::Refused(Refusal::Invalid)),
        ]
    );

    // A path a policy doesn't read refuses the call, as any refusal does.
    let host = json!([traced_policy(
        1,
        json!({"path": "request.host", "op": "existing"}),
        "alert"
    )]);
    let (outcome, _) = trace_bundle(host, json!([]), 100);
    assert_eq!(
        outcome.chain.unwrap().decision,
        Decision::Refused {
            policy: "5c8f1e42-0000-4000-8000-000000000001".parse().unwrap(),
            refusal: Refusal::NotForUse
        }
    );

    // The first step is past the limit, so no path is resolved.
    let (_, recorder) = trace_bundle(policies, json!([]), 0);
    assert_eq!(
        recorder.0,
        [("01".into(), None, Verdict::Refused(Refusal::BudgetExceeded))]
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
        [(
            "02".into(),
            Some("Number(10.0)".into()),
            Verdict::DoesNotHold
        )]
    );
}

// A request whose endpoint can't be read, a session message's or a body unread's, is unbound.
#[test]
fn test_reads_an_endpoint_not_read_as_unbound() {
    let bundle = json!({
        "schema_version": "2.0",
        "content_hash": "unbound",
        "enforcement": {"mode": "enforce"},
        "policies": [],
        "bindings": [],
        "binding_fallback": "block",
        "ticket_fallback": "pass",
    });
    let bundle = compile_bundle(&parse_bundle(&bundle.to_string()).unwrap());
    for body in [
        crate::keep_body(br#"{"method": "initialize"}"#),
        BodyRead::Unread(Unread::CutShort),
    ] {
        let request = crate::observed("a.test", "POST", "/mcp", body, 0);
        let environment = Environment::of(&request, None);
        let outcome = judge(
            &bundle,
            &environment,
            JudgingMode::Enforce,
            &mut Budget::default(),
        );
        assert_eq!(outcome.fallback_denials, [FallbackDenial::Binding]);
    }
}

// A request on a budget of `budget_bytes`, its body kept outside it if past it.
fn request_within(budget_bytes: usize, body: &[u8], header: &str) -> ObservedRequest {
    let mut buffer = BodyBuffer::new(usize::MAX, &ByteBudget::new(budget_bytes));
    buffer.keep_past_budget();
    buffer.adopt(body.to_vec());
    let body = BodyRead::Whole(Arc::new(buffer.finish().unwrap()));
    let lines = vec![(b"x-a".to_vec(), header.as_bytes().to_vec())];
    let (method, path) = (b"POST".to_vec(), b"/mcp".to_vec());
    ObservedRequest::new(Vec::new(), method, path, lines, body, 0)
}

// A tree past the budget after the ticket fallback, before the binding one; headers past it in
// the walk.
#[test]
fn test_reads_a_request_past_the_budget_after_the_ticket_fallback() {
    use FallbackDenial::Ticket;
    use JudgingMode::{Enforce, Observe};
    let call = br#"{"method": "tools/call", "params": {"name": "t"}}"#;
    let long = "a".repeat(32 << 10);
    // A tree's least reservation passes 64 bytes; 16 KiB holds it, not the long header.
    let cases = [
        (64, "1", Enforce, "block", "block", &[Ticket][..], false),
        (64, "1", Observe, "block", "block", &[Ticket], true),
        (64, "1", Enforce, "pass", "block", &[], true),
        (16 << 10, &long[..], Enforce, "pass", "pass", &[], true),
    ];
    for (budget_bytes, header, mode, ticket_fallback, binding_fallback, denials, past) in cases {
        let bundle = json!({
            "schema_version": "2.0",
            "content_hash": "past-budget",
            "enforcement": {"mode": "enforce"},
            "policies": [traced_policy(1, json!({"path": "request.headers.x-a", "op": "existing"}), "block")],
            "bindings": [],
            "binding_fallback": binding_fallback,
            "ticket_fallback": ticket_fallback,
        });
        let bundle = compile_bundle(&parse_bundle(&bundle.to_string()).unwrap());
        let request = request_within(budget_bytes, call, header);
        let outcome = judge(
            &bundle,
            &Environment::of(&request, None),
            mode,
            &mut Budget::default(),
        );
        let got = (
            &outcome.fallback_denials[..],
            outcome.past_budget,
            outcome.chain,
        );
        assert_eq!(
            got,
            (denials, past, None),
            "{mode:?} {ticket_fallback} {budget_bytes}"
        );
    }
}
