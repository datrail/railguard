use rail_model::condition::{
    ConditionProblem, ConditionUse, LocationSegment, OperandShape, ProblemCode, ProblemScope,
    TicketClaimsSchema, check_condition,
};
use rail_model::loc;
use serde_json::{Value, json};

fn get_schema() -> TicketClaimsSchema {
    TicketClaimsSchema::from_value(&json!({
        "agent_id": "string",
        "skills": "list",
        "posture": {"overall": {"score": "number"}, "admin": "bool"},
    }))
    .unwrap()
}

#[test]
fn test_says_which_operand_shape_an_op_takes() {
    let problems = check_condition(
        &json!({"path": "ticket.skills", "op": "in", "value": "x"}).to_string(),
        ConditionUse::GatewayPolicy,
        &get_schema(),
    );
    assert_eq!(
        problems,
        [ConditionProblem {
            location: loc!["value"],
            code: ProblemCode::OperandShape,
            scope: ProblemScope::Enforcement,
            expected_shape: Some(OperandShape::List),
            message: "in takes a list".to_string(),
        }]
    );
}

#[test]
fn test_quotes_input_safely() {
    let problems = check_condition(
        &json!({"path": "ticket.a\nb", "op": "existing"}).to_string(),
        ConditionUse::GatewayPolicy,
        &get_schema(),
    );
    assert_eq!(
        problems[0].message,
        "\"ticket.a\\u{a}b\": names nothing a ticket carries"
    );
}

#[test]
fn test_refuses_a_malformed_schema() {
    for (schema, refusal) in [
        (json!("string"), "a ticket claims schema is an object"),
        (json!(["string"]), "a ticket claims schema is an object"),
        (
            json!({"a": "header"}),
            r#"kind "header": not string, number, bool or list"#,
        ),
        (json!({"a": 1}), "a ticket claims schema's leaves are kinds"),
        (
            json!({"a": ["string"]}),
            "a ticket claims schema's leaves are kinds",
        ),
    ] {
        assert_eq!(
            TicketClaimsSchema::from_value(&schema).unwrap_err(),
            refusal,
            "{schema}"
        );
    }
}

#[test]
fn test_builds_a_location_from_keys_and_indexes() {
    assert_eq!(
        loc!["value", 1],
        [
            LocationSegment::Key("value".to_string()),
            LocationSegment::Index(1)
        ]
    );
    assert_eq!(
        serde_json::to_value(loc!["value", 1]).unwrap(),
        json!(["value", 1])
    );
}

#[test]
fn test_checks_a_condition_as_a_capture_plan_reads_it() {
    use ProblemCode::{OpNotForPath, OperandShape};
    use ProblemScope::Authoring;
    let problems = |condition: Value| {
        check_condition(
            &condition.to_string(),
            ConditionUse::CapturePlan,
            &get_schema(),
        )
    };
    assert_eq!(
        problems(json!({"path": ["request", "host"], "op": "in", "value": []})),
        [ConditionProblem {
            location: loc!["value"],
            code: OperandShape,
            scope: Authoring,
            expected_shape: Some(rail_model::condition::OperandShape::List),
            message: "in takes at least one value in a plan's match".to_string(),
        }]
    );
    assert_eq!(
        problems(json!({"path": ["request", "host"], "op": "lt", "value": 1})),
        [ConditionProblem {
            location: loc!["op"],
            code: OpNotForPath,
            scope: Authoring,
            expected_shape: None,
            message: "lt can't hold for a string".to_string(),
        }]
    );
}
