//! Every conformance vector, run by its layer; `-- vectors::` runs them alone.

use std::collections::{BTreeMap, HashSet};
use std::fs;

use rail_model::conformance::{FILES, SHARED, run_vectors};
use serde::Deserialize;

mod bundle;
mod check;
mod judge;
mod plan;
mod redaction;
mod ticket;

const VECTORS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vectors");

// The files whose cases their own tests read, in the same format.
const OWN_TESTS: [&str; 6] = [
    "redaction.json",
    "policy/bundle.json",
    "policy/judge.json",
    "policy/base64_ticket.json",
    "policy/jwt_ticket.json",
    "capture/plan.json",
];

fn read_texts() -> BTreeMap<String, String> {
    (FILES.iter().chain(&SHARED).chain(&OWN_TESTS))
        .map(|name| {
            let text = fs::read_to_string(format!("{VECTORS}/{name}")).unwrap();
            ((*name).to_owned(), text)
        })
        .collect()
}

#[derive(Deserialize)]
struct Ids {
    cases: Vec<Id>,
}

// Only the id read; the rest, some of it past what a `Value` holds, skipped unread.
#[derive(Deserialize)]
struct Id {
    id: String,
}

// So a doc or a proposal can name a case by its id alone.
#[test]
fn test_ids_are_unique_across_files() {
    let texts = read_texts();
    let mut seen = HashSet::new();
    for file in FILES.iter().chain(&OWN_TESTS) {
        let ids: Ids = serde_json::from_str(&texts[*file]).unwrap();
        for case in ids.cases {
            assert!(seen.insert(case.id.clone()), "{file}: {} repeated", case.id);
        }
    }
}

#[test]
fn test_every_file_of_cases() {
    let texts = read_texts();
    let mut failures = Vec::new();
    for file in FILES {
        let outcomes = run_vectors(file, &texts).unwrap();
        assert!(!outcomes.is_empty(), "{file}: no cases");
        failures.extend(
            (outcomes.iter())
                .filter(|outcome| outcome.got != outcome.expect)
                .map(|outcome| {
                    format!(
                        "{file} {}:\n  want {}\n  got  {}",
                        outcome.id, outcome.expect, outcome.got
                    )
                }),
        );
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
