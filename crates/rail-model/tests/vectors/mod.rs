//! Every conformance vector, run by its layer; `-- vectors::` runs them alone.

use std::collections::HashSet;
use std::path::Path;

use rail_model::conformance::run_vectors;

const VECTORS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vectors");

// Every file the runner reads.
const FILES: [&str; 10] = [
    "request_line.json",
    "mcp.json",
    "condition/path.json",
    "condition/condition.json",
    "condition/op.json",
    "capture/entry.json",
    "capture/record.json",
    "capture/matching.json",
    "capture/window.json",
    "capture/windowing.json",
];

// So a doc or a proposal can name a case by its id alone.
#[test]
fn test_ids_are_unique_across_files() {
    let mut seen = HashSet::new();
    for file in FILES {
        for outcome in run_vectors(Path::new(VECTORS), file).unwrap() {
            assert!(
                seen.insert(outcome.id.clone()),
                "{file}: {} repeated",
                outcome.id
            );
        }
    }
}

fn check_vectors(file: &str) {
    let outcomes = run_vectors(Path::new(VECTORS), file).unwrap();
    assert!(!outcomes.is_empty(), "{file}: no cases");
    let failures: Vec<String> = outcomes
        .iter()
        .filter(|outcome| outcome.got != outcome.expect)
        .map(|outcome| {
            format!(
                "{}:\n  want {}\n  got  {}",
                outcome.id, outcome.expect, outcome.got
            )
        })
        .collect();
    assert!(failures.is_empty(), "{file}:\n{}", failures.join("\n"));
}

#[test]
fn test_request_line() {
    check_vectors("request_line.json");
}

#[test]
fn test_mcp() {
    check_vectors("mcp.json");
}

#[test]
fn test_path() {
    check_vectors("condition/path.json");
}

#[test]
fn test_op() {
    check_vectors("condition/op.json");
}

#[test]
fn test_condition() {
    check_vectors("condition/condition.json");
}

#[test]
fn test_entry() {
    check_vectors("capture/entry.json");
}

#[test]
fn test_record() {
    check_vectors("capture/record.json");
}

#[test]
fn test_matching() {
    check_vectors("capture/matching.json");
}

#[test]
fn test_window() {
    check_vectors("capture/window.json");
}

#[test]
fn test_windowing() {
    check_vectors("capture/windowing.json");
}
