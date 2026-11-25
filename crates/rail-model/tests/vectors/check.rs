use rail_model::condition::{
    Condition, ConditionUse, ProblemCode, ProblemScope, TicketClaimsSchema, check_condition,
};
use serde::Deserialize;
use serde_json::json;
use serde_json::value::RawValue;

const CONDITION_VECTORS: &str =
    include_str!("../../../../vectors/judge_condition_conformance_cases.json");
const BUNDLE_VECTORS: &str = include_str!("../../../../vectors/policy/bundle.json");

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
#[test]
fn test_scopes_match_how_the_parser_reads_each_vector_condition() {
    let conditions = get_vector_conditions();
    assert!(conditions.len() > 300, "{}", conditions.len());
    // Every ticket path is authoring scope, so an empty schema changes no enforcement problem.
    let schema = TicketClaimsSchema::from_value(&json!({})).unwrap();

    for text in conditions {
        let problems = check_condition(&text, ConditionUse::GatewayPolicy, &schema);
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
