use rail_policy::{Condition, Path, Predicate, Scalar, Source};
use serde_json::{Value, json};

fn read(condition: Value) -> Condition {
    serde_json::from_value(condition).unwrap()
}

fn ticket_path() -> Path {
    Path {
        source: Source::Ticket,
        keys: vec!["sandbox_type".to_string()],
    }
}

#[test]
fn test_reads_every_op() {
    let path = "ticket.sandbox_type";
    let cases = [
        (json!({"path": path, "op": "existing"}), "Existing"),
        (json!({"path": path, "op": "not_existing"}), "NotExisting"),
        (json!({"path": path, "op": "eq", "value": "a"}), "Eq"),
        (json!({"path": path, "op": "ne", "value": 1}), "Ne"),
        (json!({"path": path, "op": "lt", "value": 1}), "Lt"),
        (json!({"path": path, "op": "lte", "value": 1.5}), "Lte"),
        (json!({"path": path, "op": "gt", "value": 1}), "Gt"),
        (json!({"path": path, "op": "gte", "value": 1}), "Gte"),
        (
            json!({"path": path, "op": "in", "value": ["a", 1, true]}),
            "In",
        ),
        (
            json!({"path": path, "op": "contains", "value": "a"}),
            "Contains",
        ),
        (
            json!({"path": path, "op": "not_contains", "value": 1}),
            "NotContains",
        ),
        (
            json!({"path": path, "op": "matches", "value": "a*"}),
            "Matches",
        ),
    ];
    for (condition, variant) in cases {
        let read = format!("{:?}", read(condition));
        assert!(read.starts_with(&format!("Valid({variant} {{")), "{read}");
    }
}

#[test]
fn test_reads_a_scalar_operand() {
    assert_eq!(
        read(json!({"path": "ticket.sandbox_type", "op": "eq", "value": "a"})),
        Condition::Valid(Predicate::Eq {
            path: ticket_path(),
            value: Scalar::String("a".to_string()),
        })
    );
}

#[test]
fn test_reads_an_integer_past_u64_as_a_double() {
    let text = r#"{"path": "ticket.a", "op": "lt", "value": 18446744073709551616}"#;
    let condition: Condition = serde_json::from_str(text).unwrap();
    let Condition::Valid(Predicate::Lt { value, .. }) = condition else {
        panic!("{condition:?}");
    };
    assert_eq!(value, 18446744073709551616.0);
}

#[test]
fn test_reads_a_bare_source() {
    let condition = read(json!({"path": "ticket", "op": "not_existing"}));
    let Condition::Valid(Predicate::NotExisting { path }) = condition else {
        panic!("{condition:?}");
    };
    assert!(path.keys.is_empty());
}

#[test]
fn test_ignores_extra_keys() {
    let condition = read(json!({"path": "ticket.a", "op": "existing", "value": 1, "note": "x"}));
    assert!(matches!(
        condition,
        Condition::Valid(Predicate::Existing { .. })
    ));
}

#[test]
fn test_reads_an_op_it_does_not_know_as_unknown() {
    let cases = [
        json!({"path": "ticket.a", "op": "unknown", "value": 1}),
        json!({"path": "session.a", "op": "unknown"}),
        json!({"op": ""}),
    ];
    for condition in cases {
        assert_eq!(read(condition.clone()), Condition::Unknown(condition));
    }
}

#[test]
fn test_reads_anything_else_as_invalid() {
    let cases = [
        json!("not an object"),
        json!(["lt", "ticket.a", 5]),
        json!(null),
        json!({}),
        json!({"path": "ticket.a"}),
        json!({"op": "existing"}),
        json!({"path": 1, "op": "existing"}),
        json!({"path": "ticket.a", "op": 1}),
        json!({"path": "ticket.a", "op": true}),
        json!({"path": "session.a", "op": "existing"}),
        json!({"path": "ticket.a", "op": "lt", "value": "1"}),
        json!({"path": "ticket.a", "op": "lt"}),
        json!({"path": "ticket.a", "op": "eq", "value": true}),
        json!({"path": "ticket.a", "op": "eq", "value": null}),
        json!({"path": "ticket.a", "op": "in", "value": "a"}),
        json!({"path": "ticket.a", "op": "matches", "value": 1}),
    ];
    for condition in cases {
        assert_eq!(read(condition.clone()), Condition::Invalid(condition));
    }
}
