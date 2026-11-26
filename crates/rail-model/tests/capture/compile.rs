use rail_model::capture::{compile_plan, parse_plan};
use rail_model::condition::{
    ConditionUse, LocationSegment, ProblemCode, ProblemScope, TicketClaimsSchema, check_condition,
};
use serde::Deserialize;
use serde_json::{Value, json};

const VECTORS: &str = include_str!("../../../../vectors/capture/matching.json");
const PLANS: &str = include_str!("../../../../vectors/capture/plans.json");

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    plan: Value,
    entries: Option<Value>,
}

// A case's plan: written whole, or named, its entries in place of the named plan's.
fn get_plan(plans: &Value, case: &Case) -> Value {
    let Value::String(name) = &case.plan else {
        return case.plan.clone();
    };
    let mut plan = plans["plans"][name.as_str()].clone();
    if let Some(entries) = &case.entries {
        plan["entries"] = entries.clone();
    }
    plan
}

// Rail Center checks a condition as the proxy compiles it: the first an entry refuses is the first
// every reader refuses, or whose op this one doesn't know.
#[test]
fn test_refuses_the_conditions_check_condition_says_every_reader_refuses() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    let plans: Value = serde_json::from_str(PLANS).unwrap();
    let schema = TicketClaimsSchema::from_value(&json!({})).unwrap();
    let refused_by_readers = |condition: &Value| {
        check_condition(&condition.to_string(), ConditionUse::CapturePlan, &schema)
            .iter()
            .any(|problem| {
                problem.scope == ProblemScope::Enforcement || problem.code == ProblemCode::UnknownOp
            })
    };
    assert!(!vectors.cases.is_empty());
    let mut failures = Vec::new();
    for case in &vectors.cases {
        let plan = get_plan(&plans, case);
        let compiled = compile_plan(parse_plan(&plan.to_string()).unwrap());
        let written = plan["entries"].as_array().unwrap();
        let ids: Vec<&str> = compiled
            .plan()
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        let written_ids: Vec<&str> = written
            .iter()
            .map(|entry| entry["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, written_ids, "{}", case.id);
        for (entry, written) in compiled.plan().entries.iter().zip(written) {
            let refused = compiled
                .refused()
                .find(|refused| refused.id() == entry.id)
                .and_then(|refused| refused.refusal())
                .map(|refusal| refusal.location.clone());
            let first = written["match"]
                .as_array()
                .unwrap()
                .iter()
                .position(refused_by_readers)
                .map(|position| vec![LocationSegment::Index(position)]);
            if first != refused {
                failures.push(format!("{}: {}: {first:?}", case.id, entry.id));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
