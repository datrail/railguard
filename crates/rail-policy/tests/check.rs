use rail_policy::{
    Condition, ConditionProblem, LocationSegment, OperandShape, ProblemCode, ProblemScope,
    TicketClaimsSchema, check_condition, loc,
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

fn get_schema() -> TicketClaimsSchema {
    TicketClaimsSchema::from_value(&json!({
        "agent_id": "string",
        "skills": "list",
        "posture": {"overall": {"score": "number"}, "admin": "bool"},
    }))
    .unwrap()
}

fn check(condition: Value) -> Vec<(Vec<LocationSegment>, ProblemCode, ProblemScope)> {
    check_condition(&condition.to_string(), &get_schema())
        .into_iter()
        .map(|problem| (problem.location, problem.code, problem.scope))
        .collect()
}

#[test]
fn test_scopes_match_how_the_parser_reads_each_vector_condition() {
    let conditions = get_vector_conditions();
    assert!(conditions.len() > 300, "{}", conditions.len());
    // Every ticket path is authoring scope, so an empty schema changes no enforcement problem.
    let schema = TicketClaimsSchema::from_value(&json!({})).unwrap();

    for text in conditions {
        let problems = check_condition(&text, &schema);
        let refused = problems
            .iter()
            .any(|p| p.scope == ProblemScope::Enforcement);
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
    use ProblemScope::Enforcement;
    assert_eq!(check(json!([1])), [(loc![], NotAnObject, Enforcement)]);
    assert_eq!(
        check(json!({"path": 1, "op": "existing"})),
        [(loc!["path"], UnknownSource, Enforcement)]
    );
    assert_eq!(
        check(json!({"path": "body.x", "op": "existing"})),
        [(loc!["path"], UnknownSource, Enforcement)]
    );
    assert_eq!(
        check(json!({"path": "request.method", "op": "existing"})),
        [(loc!["path"], UnknownPath, Enforcement)]
    );
    assert_eq!(
        check(json!({"path": "ticket.agent_id"})),
        [(loc!["op"], UnknownOp, Enforcement)]
    );
    assert_eq!(
        check(json!({"path": "ticket.agent_id", "op": "eq", "value": true})),
        [(loc!["value"], OperandShape, Enforcement)]
    );
}

#[test]
fn test_reports_what_only_the_writer_refuses() {
    use ProblemCode::{OpNotForPath, TooDeep, UnknownOp, UnknownPath};
    use ProblemScope::Authoring;
    assert_eq!(
        check(json!({"path": "ticket.posture.score", "op": "existing"})),
        [(loc!["path"], UnknownPath, Authoring)]
    );
    assert_eq!(
        check(json!({"path": "request.headers.X-A", "op": "existing"})),
        [(loc!["path"], UnknownPath, Authoring)]
    );
    assert_eq!(
        check(json!({"path": "request.headers.", "op": "existing"})),
        [(loc!["path"], UnknownPath, Authoring)]
    );
    assert_eq!(
        check(json!({"path": "ticket.agent_id", "op": "lt", "value": 1})),
        [(loc!["op"], OpNotForPath, Authoring)]
    );
    assert_eq!(
        check(json!({"path": "ticket.posture.admin", "op": "eq", "value": 1})),
        [(loc!["op"], OpNotForPath, Authoring)]
    );
    assert_eq!(
        check(json!({"path": "ticket.skills.agent_id", "op": "existing"})),
        [(loc!["path"], UnknownPath, Authoring)]
    );
    // Header names are lowercased as Unicode, so `É` isn't the `é` a reader would look up.
    assert_eq!(
        check(json!({"path": "request.headers.É", "op": "existing"})),
        [(loc!["path"], UnknownPath, Authoring)]
    );
    // 32 levels deep is accepted, 33 refused.
    let mut deep = json!(1);
    for _ in 0..30 {
        deep = json!([deep]);
    }
    assert_eq!(
        check(json!({"path": "ticket.skills", "op": "in", "value": deep.clone()})),
        []
    );
    assert_eq!(
        check(json!({"path": "ticket.skills", "op": "in", "value": [deep]})),
        [(loc![], TooDeep, Authoring)]
    );
    // An unknown op may be a newer reader's, with a path only it knows.
    assert_eq!(
        check(json!({"path": "body.x", "op": "near", "value": 1})),
        [
            (loc!["op"], UnknownOp, Authoring),
            (loc!["path"], ProblemCode::UnknownSource, Authoring)
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
fn test_reports_json_this_reader_cannot_hold() {
    let text = r#"{"path": "ticket.posture.overall.score", "op": "lt", "value": 1e400}"#;
    let problems = check_condition(text, &get_schema());
    assert_eq!(problems.len(), 1);
    assert_eq!(
        (problems[0].code, problems[0].scope),
        (ProblemCode::Unreadable, ProblemScope::Enforcement)
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
fn test_allows_each_op_only_where_it_can_hold() {
    // Rail Center's `_OPS_FOR`, beside `existing` and `not_existing`, which every kind takes.
    let cases = [
        ("ticket", &[][..]),
        ("ticket.posture", &[]),
        ("ticket.posture.admin", &[]),
        ("ticket.agent_id", &["eq", "ne", "in", "matches"]),
        (
            "ticket.posture.overall.score",
            &["eq", "ne", "lt", "lte", "gt", "gte", "in"],
        ),
        (
            "ticket.skills",
            &["eq", "ne", "in", "matches", "contains", "not_contains"],
        ),
        (
            "request.headers.x-a",
            &["eq", "ne", "in", "matches", "contains", "not_contains"],
        ),
        ("request.endpoint_key", &["eq", "ne", "in", "matches"]),
        ("request.headers", &[]),
    ];
    let ops = [
        "existing",
        "not_existing",
        "eq",
        "ne",
        "lt",
        "lte",
        "gt",
        "gte",
        "in",
        "contains",
        "not_contains",
        "matches",
    ];
    for (path, allowed) in cases {
        for op in ops {
            let found = check(json!({"path": path, "op": op}))
                .iter()
                .any(|(_, code, _)| *code == ProblemCode::OpNotForPath);
            let held = op.ends_with("existing") || allowed.contains(&op);
            assert_eq!(found, !held, "{path} {op}");
        }
    }
}

#[test]
fn test_reads_any_path_problem_as_authoring_beside_an_unknown_op() {
    use ProblemCode::{UnknownOp, UnknownPath, UnknownSource};
    use ProblemScope::Authoring;
    assert_eq!(
        check(json!({"path": "request.method", "op": "near"})),
        [
            (loc!["op"], UnknownOp, Authoring),
            (loc!["path"], UnknownPath, Authoring)
        ]
    );
    assert_eq!(
        check(json!({"path": 1, "op": "near"})),
        [
            (loc!["op"], UnknownOp, Authoring),
            (loc!["path"], UnknownSource, Authoring)
        ]
    );
}

#[test]
fn test_reports_what_a_reader_parses_before_what_only_rail_center_checks() {
    for (path, problem) in [
        ("ticket.nope", (loc!["path"], ProblemCode::UnknownPath)),
        ("ticket.agent_id", (loc!["op"], ProblemCode::OpNotForPath)),
    ] {
        assert_eq!(
            check(json!({"path": path, "op": "lt", "value": "x"})),
            [
                (
                    loc!["value"],
                    ProblemCode::OperandShape,
                    ProblemScope::Enforcement
                ),
                (problem.0, problem.1, ProblemScope::Authoring)
            ]
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
