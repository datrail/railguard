//! Every conformance vector, run by its layer; `-- vectors::` runs them alone.

use std::collections::{BTreeMap, HashSet};
use std::fs;

use rail_model::conformance::{FILES, SHARED, run_vectors};
use serde::Deserialize;

mod bundle;
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

// A case whose `expect` is wrong comes out unequal, so no outcome passes for being blank.
#[test]
fn test_a_wrong_expect_is_told() {
    let mut texts = read_texts();
    let case = r#"{"cases": [{"id": "a", "authority": "mcp.example.com", "method": "POST", "path": "/mcp",
        "expect": {"host": {"read": "mcp.example.com"}, "method": "POST", "reported_method": "POST",
        "path": "/mcp", "redacted_path": "/mcp", "segments": ["mcp"], "redacted_segments": ["mcp"],
        "query": {}, "unmatchable": "bracket"}}]}"#;
    texts.insert("request_line.json".to_owned(), case.to_owned());
    let outcomes = run_vectors("request_line.json", &texts).unwrap();
    assert_ne!(outcomes[0].got, outcomes[0].expect);
    assert_eq!(outcomes[0].got["unmatchable"], serde_json::Value::Null);
    assert_eq!(outcomes[0].got["segments"], outcomes[0].expect["segments"]);
}

// A case whose inputs contradict each other fails its file, so none is silently run another way.
#[test]
fn test_a_malformed_case_is_refused() {
    let call = |extra: &str| {
        format!(
            r#"{{"authority": "a.example", "method": "POST", "path": "/mcp", "body": {{"unread": "not_sent"}}, "body_bytes": 0{extra}}}"#
        )
    };
    let matching = |plan: &str, extra: &str| {
        format!(
            r#"{{"cases": [{{"id": "a", "plan": {plan}, "entries": [], "call": {}, "expect": null}}]}}"#,
            call(extra)
        )
    };
    let mut texts = read_texts();
    let whole_plan = r#"{"schema_version": "1.0"}"#;
    for (case, why) in [
        (
            matching(r#""matching""#, r#", "byte_budget": 1"#),
            "byte_budget",
        ),
        (
            matching(whole_plan, ""),
            "entries beside a plan written whole",
        ),
    ] {
        texts.insert("capture/matching.json".to_owned(), case);
        let refused = run_vectors("capture/matching.json", &texts).unwrap_err();
        assert!(refused.contains(why), "{refused}");
    }
}
