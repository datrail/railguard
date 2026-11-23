use std::collections::BTreeMap;

use rail_model::capture::{CompiledPlan, EntryRefusal, compile_plan, parse_plan};
use serde::Deserialize;
use serde_json::Value;

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

fn describe(refusal: EntryRefusal) -> String {
    match refusal {
        EntryRefusal::Condition { position, refusal } => {
            let refusal = serde_json::to_value(refusal).unwrap();
            format!("condition {position}: {}", refusal.as_str().unwrap())
        }
        EntryRefusal::NotOneEndpoint => "not_one_endpoint".to_string(),
    }
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
        let mut plan = vectors.plan.clone();
        case.patch
            .iter()
            .for_each(|operation| patch(&mut plan, operation));
        let compiled = compile_plan(parse_plan(&plan.to_string()).unwrap());
        for difference in compare(&compiled, &case.expect) {
            failures.push(format!("{}: {difference}", case.name));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn test_keeps_the_plan_it_compiles() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    let compiled = compile_plan(parse_plan(&vectors.plan.to_string()).unwrap());
    let ids: Vec<&str> = compiled
        .plan()
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(ids, ["e1", "e2", "e3"]);
}
