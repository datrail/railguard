use rail_policy::{
    Action, BindingMode, Condition, EndpointKey, EnforcementMode, Fallback, Path, Predicate,
    SchemaVersion, TicketPath, parse_bundle,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use uuid::Uuid;

const VECTORS: &str = include_str!("../../../vectors/parse_bundle_conformance_cases.json");

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
    // Raw, since a priority may be past what a `Value` holds.
    bundle: Box<RawValue>,
    usable: bool,
    /// Usable cases only: the enabled policy ids, in evaluation order.
    chain: Option<Vec<Uuid>>,
    /// Usable cases only: the priorities of `chain`, in the same order.
    chain_priorities: Option<Vec<i64>>,
}

#[test]
fn test_vectors() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());

    let mut failures = Vec::new();
    for case in vectors.cases {
        let name = case.name;
        let result = parse_bundle(case.bundle.get());
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

    let version = SchemaVersion { major: 2, minor: 0 };
    assert_eq!(bundle.schema_version, version);
    assert_eq!(bundle.content_hash, "a3f1c09e7b2d4485");
    assert_eq!(bundle.enforcement.mode, EnforcementMode::Observe);
    assert_eq!(bundle.binding_fallback, Fallback::Block);
    assert_eq!(bundle.ticket_fallback, Fallback::Pass);

    let policy = &bundle.policies[0];
    assert_eq!(policy.name, "P1");
    assert_eq!(policy.action, Action::Block);
    let Condition::Valid(Predicate::Lt { path, value }) = &policy.condition else {
        panic!("{:?}", policy.condition);
    };
    let keys = ["posture", "overall", "score"].map(str::to_string).to_vec();
    assert_eq!(*path, Path::Ticket(TicketPath(keys)));
    assert_eq!(*value, 40.0);

    let binding = &bundle.bindings[0];
    assert_eq!(binding.mode, BindingMode::Gated);
    let key = EndpointKey {
        slug: "finretail".to_string(),
        resolved: "/mcp#tools/call#refund_order".parse().unwrap(),
    };
    assert_eq!(binding.endpoint_key, key);
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
fn test_reads_an_operand_as_the_nearest_double() {
    let bundle = EXAMPLE.replace(r#""value": 40"#, r#""value": 6.934698245112694e276"#);
    let bundle = parse_bundle(&bundle).unwrap();
    let Condition::Valid(Predicate::Lt { value, .. }) = &bundle.policies[0].condition else {
        panic!("{:?}", bundle.policies[0].condition);
    };
    assert_eq!(value.to_bits(), 0x79690975fbde15b0);
}

#[test]
fn test_reads_a_version_of_two_numbers() {
    let read = |version: &str| {
        let bundle = EXAMPLE.replace(r#""2.0""#, &format!(r#""{version}""#));
        parse_bundle(&bundle).map(|bundle| bundle.schema_version)
    };

    assert_eq!(
        read("2.10").unwrap(),
        SchemaVersion {
            major: 2,
            minor: 10
        }
    );
    for version in [
        "2", "2.0.0", "+2.0", "2.+0", "2.", ".0", "2.0 ", "-1.0", "2,0",
    ] {
        assert!(read(version).is_err(), "{version:?}");
    }
}

#[test]
fn test_keeps_disabled_policies() {
    let bundle = EXAMPLE.replace(r#""enabled": true"#, r#""enabled": false"#);
    let bundle = parse_bundle(&bundle).unwrap();
    assert!(!bundle.policies[0].enabled);
}

#[test]
fn test_reads_an_endpoint_key_with_a_separator_in_its_call() {
    let bundle = EXAMPLE.replace("#refund_order", "#refund#order");
    let bundle = parse_bundle(&bundle).unwrap();
    assert_eq!(
        bundle.bindings[0].endpoint_key.resolved.call(),
        "refund#order"
    );
}

#[test]
fn test_refuses_an_endpoint_key_with_a_part_missing_or_empty() {
    let keys = [
        "finretail#/mcp#tools/call",
        "#/mcp#tools/call#refund_order",
        "finretail##tools/call#refund_order",
        "finretail#/mcp##refund_order",
        "finretail#/mcp#tools/call#",
    ];
    for key in keys {
        let bundle = EXAMPLE.replace("finretail#/mcp#tools/call#refund_order", key);
        assert!(parse_bundle(&bundle).is_err(), "{key}");
    }
}

#[test]
fn test_keeps_a_bundle_whose_condition_is_unknown() {
    let bundle = EXAMPLE.replace(r#""op": "lt""#, r#""op": "unknown""#);
    let bundle = parse_bundle(&bundle).unwrap();
    assert!(matches!(
        bundle.policies[0].condition,
        Condition::Unknown(_)
    ));
}

#[test]
fn test_keeps_a_bundle_whose_condition_has_an_operand_past_a_double() {
    let big = format!("1{}", "0".repeat(400));
    let bundle = EXAMPLE.replace(r#""value": 40"#, &format!(r#""value": {big}"#));
    let bundle = parse_bundle(&bundle).unwrap();
    let Condition::Invalid(text) = &bundle.policies[0].condition else {
        panic!("{:?}", bundle.policies[0].condition);
    };
    // As sent, whitespace included.
    assert!(text.contains(&format!(r#""value": {big}"#)), "{text}");
}

#[test]
fn test_keeps_a_bundle_whose_condition_is_nested_past_any_limit() {
    let deep = format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000));
    let bundle = EXAMPLE.replace(r#""value": 40"#, &format!(r#""value": {deep}"#));
    let bundle = parse_bundle(&bundle).unwrap();
    assert!(matches!(
        bundle.policies[0].condition,
        Condition::Invalid(_)
    ));
}

#[test]
fn test_names_an_endpoint_key_as_sent_when_refusing() {
    let bundle = EXAMPLE.replace(r#""gated""#, r#""open""#);
    let refusal = parse_bundle(&bundle).unwrap_err().to_string();
    assert_eq!(
        refusal,
        "unusable policy bundle: open binding finretail#/mcp#tools/call#refund_order names policies"
    );
}
