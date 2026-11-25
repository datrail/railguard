//! Layer 6: an entry's "and", as a one-entry plan compiles it.

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use super::environment::Named;
use super::plan::OneEntry;
use super::value::encode_verdict;
use super::{Outcome, create_outcome, read_cases};
use crate::capture::answer;
use crate::condition::Budget;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(rename = "note")]
    _note: Option<String>,
    environment: String,
    #[serde(rename = "match")]
    entry_match: Box<RawValue>,
    expect: Value,
}

pub(super) fn run_cases(text: &str, environments: &Named) -> Result<Vec<Outcome>, String> {
    read_cases::<Case>(text)?
        .into_iter()
        .map(|case| {
            let sources = environments.build(&case.environment)?;
            let plan = OneEntry {
                entry_match: Some(case.entry_match.get()),
                ..OneEntry::default()
            }
            .compile()
            .map_err(|e| format!("{}: {e}", case.id))?;
            let entry = plan.entry(0).expect("one entry");
            let verdict = answer(entry, &sources.environment(), &mut Budget::default());
            Ok(create_outcome(
                &case.id,
                case.expect,
                encode_verdict(verdict),
            ))
        })
        .collect()
}
