use rail_model::capture::parse_plan;
use rail_model::condition::Condition;
use serde::Deserialize;
use serde_json::Value;

use crate::capture::patch;

const VECTORS: &str = include_str!("../../../../vectors/capture/plan.json");

#[derive(Deserialize)]
struct Vectors {
    plan: Value,
    cases: Vec<Case>,
}

fn vectors() -> Vectors {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());
    vectors
}

#[derive(Deserialize)]
struct Case {
    id: String,
    #[serde(default)]
    patch: Vec<Value>,
    expect: Expect,
}

#[derive(Deserialize)]
struct Expect {
    valid: bool,
    /// The first entry's, as read.
    conditions: Option<Vec<String>>,
}
fn condition_kind(condition: &Condition) -> &'static str {
    match condition {
        Condition::Valid(_) => "valid",
        Condition::Unknown(_) => "unknown",
        Condition::Invalid(_) => "invalid",
    }
}
#[test]
fn test_vectors() {
    let vectors = vectors();
    let mut failures = Vec::new();
    for case in &vectors.cases {
        let mut plan = vectors.plan.clone();
        case.patch
            .iter()
            .for_each(|operation| patch(&mut plan, operation));

        let read = parse_plan(&plan.to_string());

        let name = &case.id;
        match (read, case.expect.valid) {
            (Ok(plan), true) => {
                let Some(conditions) = &case.expect.conditions else {
                    continue;
                };
                let read: Vec<_> = plan.entries[0]
                    .conditions
                    .iter()
                    .map(condition_kind)
                    .collect();
                if &read != conditions {
                    failures.push(format!("{name}: conditions {read:?}"));
                }
            }
            (Err(_), false) => {}
            (read, _) => failures.push(format!("{name}: {read:?}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
