use rail_policy::{Action, BindingMode, EnforcementMode, Fallback, SchemaVersion, parse_bundle};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

const VECTORS: &str = include_str!("../../../vectors/bundle.json");

const EXAMPLE: &str = r##"{
  "schema_version": "2.0",
  "content_hash": "a3f1c09e7b2d4485",
  "generated_at": "2026-07-28T20:41:07.512247Z",
  "enforcement": { "mode": "observe" },
  "policies": [
    {
      "id": "5c8f1e42-0000-4000-8000-0000000000a1",
      "name": "P1",
      "priority": 1,
      "condition": { "path": "ticket.posture.overall.score", "op": "lt", "value": 40 },
      "action": "block",
      "enabled": true
    }
  ],
  "bindings": [
    {
      "endpoint_key": "finretail#/mcp#tools/call#refund_order",
      "mode": "gated",
      "policy_ids": ["5c8f1e42-0000-4000-8000-0000000000a1"]
    }
  ],
  "binding_fallback": "block",
  "ticket_fallback": "pass"
}"##;

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    bundle: Value,
    usable: bool,
    /// Usable cases only: the enabled policy ids, in evaluation order.
    chain: Option<Vec<Uuid>>,
    /// Usable cases only: the priorities of `chain`, in the same order.
    chain_priorities: Option<Vec<i64>>,
}

#[test]
fn test_vectors() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(vectors.cases.len() >= 100);

    let mut failures = Vec::new();
    for case in vectors.cases {
        let name = case.name;
        let result = parse_bundle(&case.bundle.to_string());
        if case.usable == result.is_err() {
            failures.push(format!("{name}: {result:?}"));
        } else if let Ok(bundle) = result {
            let (chain, priorities) = bundle
                .policies
                .iter()
                .filter(|p| p.enabled)
                .map(|p| (p.id, p.priority))
                .unzip();
            if case.chain != Some(chain) || case.chain_priorities != Some(priorities) {
                failures.push(format!("{name}: wrong chain"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn test_reads_every_field_rail_center_publishes() {
    let bundle = parse_bundle(EXAMPLE).unwrap();

    let version = SchemaVersion {
        major: 2,
        minor: 0,
        revision: 0,
    };
    assert_eq!(bundle.schema_version, version);
    assert_eq!(bundle.content_hash, "a3f1c09e7b2d4485");
    assert_eq!(bundle.enforcement.mode, EnforcementMode::Observe);
    assert_eq!(bundle.binding_fallback, Fallback::Block);
    assert_eq!(bundle.ticket_fallback, Fallback::Pass);

    let policy = &bundle.policies[0];
    assert_eq!(policy.name, "P1");
    assert_eq!(policy.action, Action::Block);
    assert_eq!(policy.condition["op"], "lt");

    let binding = &bundle.bindings[0];
    assert_eq!(binding.mode, BindingMode::Gated);
    assert_eq!(
        binding.endpoint_key,
        "finretail#/mcp#tools/call#refund_order"
    );
    assert_eq!(binding.policy_ids, [policy.id]);
}

#[test]
fn test_refuses_bindings_that_differ_only_by_slug() {
    let bundle = EXAMPLE.replace(
        r#""bindings": ["#,
        r#""bindings": [{"endpoint_key": "other#/mcp#tools/call#refund_order", "mode": "open", "policy_ids": []},"#,
    );
    assert!(parse_bundle(&bundle).is_err());
}

#[test]
fn test_keeps_bindings_that_differ_past_the_slug() {
    let bundle = EXAMPLE.replace(
        r#""bindings": ["#,
        r#""bindings": [{"endpoint_key": "finretail#/other#tools/call#refund_order", "mode": "open", "policy_ids": []},"#,
    );
    assert_eq!(parse_bundle(&bundle).unwrap().bindings.len(), 2);
}

#[test]
fn test_reads_an_operand_too_long_for_a_double() {
    let operand = "1".repeat(401);
    let bundle = EXAMPLE.replace(r#""value": 40"#, &format!(r#""value": {operand}"#));
    let bundle = parse_bundle(&bundle).unwrap();
    assert_eq!(bundle.policies[0].condition["value"].to_string(), operand);
}

#[test]
fn test_reads_a_version_of_two_or_three_parts() {
    let read = |version: &str| {
        let bundle = EXAMPLE.replace(r#""2.0""#, &format!(r#""{version}""#));
        parse_bundle(&bundle).map(|bundle| bundle.schema_version)
    };

    assert_eq!(read("2.0").unwrap().revision, 0);
    assert_eq!(read("2.1.3").unwrap().revision, 3);
    assert!(read("2").is_err());
    assert!(read("2.0.0.0").is_err());
}

#[test]
fn test_keeps_disabled_policies() {
    let bundle = EXAMPLE.replace(r#""enabled": true"#, r#""enabled": false"#);
    let bundle = parse_bundle(&bundle).unwrap();
    assert!(!bundle.policies[0].enabled);
}
