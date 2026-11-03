use rail_policy::{Condition, Path, Predicate, Scalar, TicketPath};
use serde_json::{Value, json};

fn read(condition: Value) -> Condition {
    serde_json::from_str(&condition.to_string()).unwrap()
}

fn ticket_path() -> Path {
    Path::Ticket(TicketPath(vec!["sandbox_type".to_string()]))
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
    let text = r#"{"path": "ticket.sandbox_type", "op": "lt", "value": 18446744073709551616}"#;
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
    assert_eq!(path, Path::Ticket(TicketPath(vec![])));
}

#[test]
fn test_reads_any_path_under_ticket_as_its_keys() {
    let condition = read(json!({"path": "ticket.posture_score.a", "op": "existing"}));
    let Condition::Valid(Predicate::Existing { path }) = condition else {
        panic!("{condition:?}");
    };
    let keys = vec!["posture_score".to_string(), "a".to_string()];
    assert_eq!(path, Path::Ticket(TicketPath(keys)));
}

#[test]
fn test_reads_what_serde_json_cannot_hold_as_invalid() {
    let big = format!("1{}", "0".repeat(400));
    let deep = format!("{}1{}", "[".repeat(200), "]".repeat(200));
    let cases = [
        format!(r#"{{"path": "ticket.posture.overall.score", "op": "lt", "value": {big}}}"#),
        format!(r#"{{"path": "ticket.posture.overall.score", "op": "lt", "value": -{big}}}"#),
        r#"{"path": "ticket.posture.overall.score", "op": "lt", "value": 1e400}"#.to_string(),
        format!(r#"{{"path": "ticket.skills", "op": "in", "value": [1, {big}]}}"#),
        format!(r#"{{"path": "ticket.skills", "op": "in", "value": {deep}}}"#),
        r#"{"path": "ticket.sandbox_type", "op": "eq", "value": "\ud800"}"#.to_string(),
    ];
    for text in cases {
        let condition: Condition = serde_json::from_str(&text).unwrap();
        assert_eq!(condition, Condition::Invalid(text));
    }
}

#[test]
fn test_ignores_extra_keys() {
    let condition =
        read(json!({"path": "ticket.sandbox_type", "op": "existing", "value": 1, "note": "x"}));
    assert!(matches!(
        condition,
        Condition::Valid(Predicate::Existing { .. })
    ));
}

#[test]
fn test_reads_an_op_it_does_not_know_as_unknown() {
    let cases = [
        json!({"path": "ticket.sandbox_type", "op": "unknown", "value": 1}),
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
        json!(["lt", "ticket.sandbox_type", 5]),
        json!(null),
        json!({}),
        json!({"path": "ticket.sandbox_type"}),
        json!({"op": "existing"}),
        json!({"path": 1, "op": "existing"}),
        json!({"path": "ticket.sandbox_type", "op": 1}),
        json!({"path": "ticket.sandbox_type", "op": true}),
        json!({"path": "session.a", "op": "existing"}),
        json!({"path": "ticket.sandbox_type", "op": "lt", "value": "1"}),
        json!({"path": "ticket.sandbox_type", "op": "lt"}),
        json!({"path": "ticket.sandbox_type", "op": "eq", "value": true}),
        json!({"path": "ticket.sandbox_type", "op": "eq", "value": null}),
        json!({"path": "ticket.sandbox_type", "op": "in", "value": "a"}),
        json!({"path": "ticket.sandbox_type", "op": "matches", "value": 1}),
    ];
    for condition in cases {
        assert_eq!(
            read(condition.clone()),
            Condition::Invalid(condition.to_string())
        );
    }
}
