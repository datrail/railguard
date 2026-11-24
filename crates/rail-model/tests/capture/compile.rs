use std::collections::BTreeMap;

use rail_model::capture::{CompiledPlan, EntryRefusal, compile_plan, parse_plan};
use rail_model::condition::{
    ConditionUse, LocationSegment, ProblemCode, ProblemScope, TicketClaimsSchema, check_condition,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::patch;

const VECTORS: &str = include_str!("../../../../vectors/compile_plan_conformance_cases.json");

#[derive(Deserialize)]
struct Vectors {
    plan: Value,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    patch: Vec<Value>,
    expect: Expect,
}

#[derive(Deserialize)]
struct Expect {
    refused: BTreeMap<String, String>,
    entries_skipped: usize,
}

fn describe(refusal: &EntryRefusal) -> String {
    let [LocationSegment::Index(position)] = refusal.location.as_slice() else {
        panic!("{refusal:?}");
    };
    let why = serde_json::to_value(refusal.refusal).unwrap();
    format!("condition {position}: {}", why.as_str().unwrap())
}

fn compare(compiled: &CompiledPlan, expect: &Expect) -> Vec<String> {
    let mut differences = Vec::new();
    let refused: BTreeMap<String, String> = compiled
        .refused()
        .map(|entry| (entry.id().to_string(), describe(entry.refusal().unwrap())))
        .collect();
    if refused != expect.refused {
        differences.push(format!("refused {refused:?}"));
    }
    if compiled.entries_skipped() != expect.entries_skipped {
        differences.push(format!("entries_skipped {}", compiled.entries_skipped()));
    }
    differences
}

#[test]
fn test_vectors() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());
    let mut failures = Vec::new();
    for case in &vectors.cases {
        let plan = patched(&vectors, case);
        let compiled = compile_plan(parse_plan(&plan.to_string()).unwrap());
        for difference in compare(&compiled, &case.expect) {
            failures.push(format!("{}: {difference}", case.name));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn patched(vectors: &Vectors, case: &Case) -> Value {
    let mut plan = vectors.plan.clone();
    case.patch
        .iter()
        .for_each(|operation| patch(&mut plan, operation));
    plan
}

// Rail Center checks a condition as the proxy compiles it: the first an entry refuses is the first
// every reader refuses, or whose op this one doesn't know.
#[test]
fn test_refuses_the_conditions_check_condition_says_every_reader_refuses() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    let schema = TicketClaimsSchema::from_value(&json!({})).unwrap();
    let refused_by_readers = |condition: &Value| {
        check_condition(&condition.to_string(), ConditionUse::CapturePlan, &schema)
            .iter()
            .any(|problem| {
                problem.scope == ProblemScope::Enforcement || problem.code == ProblemCode::UnknownOp
            })
    };
    let mut failures = Vec::new();
    for case in &vectors.cases {
        let plan = patched(&vectors, case);
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
        assert_eq!(ids, written_ids, "{}", case.name);
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
                failures.push(format!("{}: {}: {first:?}", case.name, entry.id));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
