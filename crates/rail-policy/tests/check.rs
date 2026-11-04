use rail_policy::{
    Condition, OperandShape, Problem, ProblemCode, Scope, TicketSchema, check_condition,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

const CONDITION_VECTORS: &str =
    include_str!("../../../vectors/judge_condition_conformance_cases.json");
const BUNDLE_VECTORS: &str = include_str!("../../../vectors/parse_bundle_conformance_cases.json");

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    condition: Option<Box<RawValue>>,
    bundle: Option<Box<RawValue>>,
}

#[derive(Deserialize)]
struct LooseBundle {
    policies: Vec<LoosePolicy>,
}

#[derive(Deserialize)]
struct LoosePolicy {
    condition: Box<RawValue>,
}

/// Each condition in the vectors, as sent.
fn get_vector_conditions() -> Vec<String> {
    let mut conditions = Vec::new();
    for text in [CONDITION_VECTORS, BUNDLE_VECTORS] {
        let vectors: Vectors = serde_json::from_str(text).unwrap();
        for case in vectors.cases {
            conditions.extend(case.condition.map(|raw| raw.get().to_string()));
            let bundle = case
                .bundle
                .and_then(|raw| serde_json::from_str(raw.get()).ok());
            if let Some(LooseBundle { policies }) = bundle {
                conditions.extend(policies.iter().map(|p| p.condition.get().to_string()));
            }
        }
    }
    conditions
}

fn get_schema() -> TicketSchema {
    TicketSchema::from_value(&json!({
        "agent_id": "string",
        "skills": "list",
        "posture": {"overall": {"score": "number"}, "admin": "boolean"},
    }))
    .unwrap()
}

fn check(condition: Value) -> Vec<(&'static str, ProblemCode, Scope)> {
    check_condition(&condition.to_string(), &get_schema())
        .into_iter()
        .map(|problem| (problem.pointer, problem.code, problem.scope))
        .collect()
}

#[test]
fn test_scopes_match_how_the_parser_reads_each_vector_condition() {
    let conditions = get_vector_conditions();
    assert!(conditions.len() > 300, "{}", conditions.len());
    // Every ticket path is authoring scope, so an empty schema changes no enforcement problem.
    let schema = TicketSchema::from_value(&json!({})).unwrap();

    for text in conditions {
        let problems = check_condition(&text, &schema);
        let refused = problems.iter().any(|p| p.scope == Scope::Enforcement);
        let unknown = !refused && problems.iter().any(|p| p.code == ProblemCode::UnknownOp);
        let condition: Condition = serde_json::from_str(&text).unwrap();
        let reading = match condition {
            Condition::Valid(_) => "valid",
            Condition::Unknown(_) => "unknown",
            Condition::Invalid(_) => "invalid",
        };
        let checked = match (refused, unknown) {
            (true, _) => "invalid",
            (false, true) => "unknown",
            (false, false) => "valid",
        };
        assert_eq!(checked, reading, "{text}: {problems:?}");
    }
}

#[test]
fn test_accepts_a_condition_every_reader_reads() {
    assert_eq!(
        check(json!({"path": "ticket.posture.overall.score", "op": "lt", "value": 40})),
        []
    );
    assert_eq!(
        check(json!({"path": "request.headers.x-a", "op": "contains", "value": "b"})),
        []
    );
    assert_eq!(
        check(json!({"path": "ticket.posture", "op": "existing"})),
        []
    );
}

#[test]
fn test_reports_what_every_reader_refuses() {
    use ProblemCode::{NotAnObject, OperandShape, UnknownOp, UnknownPath, UnknownSource};
    use Scope::Enforcement;
    assert_eq!(check(json!([1])), [("", NotAnObject, Enforcement)]);
    assert_eq!(
        check(json!({"path": 1, "op": "existing"})),
        [("/path", UnknownSource, Enforcement)]
    );
    assert_eq!(
        check(json!({"path": "body.x", "op": "existing"})),
        [("/path", UnknownSource, Enforcement)]
    );
    assert_eq!(
        check(json!({"path": "request.method", "op": "existing"})),
        [("/path", UnknownPath, Enforcement)]
    );
    assert_eq!(
        check(json!({"path": "ticket.agent_id"})),
        [("/op", UnknownOp, Enforcement)]
    );
    assert_eq!(
        check(json!({"path": "ticket.agent_id", "op": "eq", "value": true})),
        [("/value", OperandShape, Enforcement)]
    );
}

#[test]
fn test_reports_what_only_the_writer_refuses() {
    use ProblemCode::{OpNotForPath, TooDeep, UnknownOp, UnknownPath};
    use Scope::Authoring;
    assert_eq!(
        check(json!({"path": "ticket.posture.score", "op": "existing"})),
        [("/path", UnknownPath, Authoring)]
    );
    assert_eq!(
        check(json!({"path": "request.headers.X-A", "op": "existing"})),
        [("/path", UnknownPath, Authoring)]
    );
    assert_eq!(
        check(json!({"path": "request.headers.", "op": "existing"})),
        [("/path", UnknownPath, Authoring)]
    );
    assert_eq!(
        check(json!({"path": "ticket.agent_id", "op": "lt", "value": 1})),
        [("/op", OpNotForPath, Authoring)]
    );
    assert_eq!(
        check(json!({"path": "ticket.posture.admin", "op": "eq", "value": 1})),
        [("/op", OpNotForPath, Authoring)]
    );
    let mut deep = json!(1);
    for _ in 0..31 {
        deep = json!([deep]);
    }
    assert_eq!(
        check(json!({"path": "ticket.skills", "op": "in", "value": deep})),
        [("", TooDeep, Authoring)]
    );
    // An unknown op may be a newer reader's, with a path only it knows.
    assert_eq!(
        check(json!({"path": "body.x", "op": "near", "value": 1})),
        [
            ("/path", ProblemCode::UnknownSource, Authoring),
            ("/op", UnknownOp, Authoring)
        ]
    );
}

#[test]
fn test_reports_every_problem_in_order() {
    assert_eq!(
        check(json!({"path": "ticket.nope", "op": "lt", "value": "x"})),
        [
            ("/path", ProblemCode::UnknownPath, Scope::Authoring),
            ("/value", ProblemCode::OperandShape, Scope::Enforcement)
        ]
    );
}

#[test]
fn test_says_which_operand_shape_an_op_takes() {
    let problems = check_condition(
        &json!({"path": "ticket.skills", "op": "in", "value": "x"}).to_string(),
        &get_schema(),
    );
    assert_eq!(
        problems,
        [Problem {
            pointer: "/value",
            code: ProblemCode::OperandShape,
            scope: Scope::Enforcement,
            expected: Some(OperandShape::List),
            message: "in takes a list".to_string(),
        }]
    );
}

#[test]
fn test_reports_json_this_reader_cannot_hold() {
    let text = r#"{"path": "ticket.posture.overall.score", "op": "lt", "value": 1e400}"#;
    let problems = check_condition(text, &get_schema());
    assert_eq!(problems.len(), 1);
    assert_eq!(
        (problems[0].code, problems[0].scope),
        (ProblemCode::Unreadable, Scope::Enforcement)
    );
}

#[test]
fn test_quotes_input_safely() {
    let problems = check_condition(
        &json!({"path": "ticket.a\nb", "op": "existing"}).to_string(),
        &get_schema(),
    );
    assert_eq!(
        problems[0].message,
        "\"ticket.a\\u{a}b\": names nothing a ticket carries"
    );
}

#[test]
fn test_refuses_a_malformed_schema() {
    assert!(TicketSchema::from_value(&json!("string")).is_err());
    assert!(TicketSchema::from_value(&json!({"a": "header"})).is_err());
    assert!(TicketSchema::from_value(&json!({"a": 1})).is_err());
}
