//! Layer 6: an entry's "and", as a one-entry plan compiles it.

use serde::Deserialize;
use serde_json::Value;

use super::environment::Named;
use super::plans::NamedPlans;
use super::value::encode_verdict;
use super::{Outcome, build_budget, read_cases, run_each};
use crate::capture::answer;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(rename = "note")]
    _note: Option<String>,
    environment: String,
    #[serde(rename = "match")]
    entry_match: Value,
    step_limit: Option<u64>,
    expect: Value,
}

pub(super) fn run_cases(
    text: &str,
    environments: &Named,
    plans: &NamedPlans,
) -> Result<Vec<Outcome>, String> {
    run_each(read_cases::<Case>(text)?, |case| {
        let got = run_case(&case, environments, plans);
        (case.id, case.expect, got)
    })
}

fn run_case(case: &Case, environments: &Named, plans: &NamedPlans) -> Result<Value, String> {
    let built = environments.build(&case.environment)?;
    let plan = plans.compile_edited("one-entry", |plan| {
        let entry_match = plan
            .pointer_mut("/entries/0/match")
            .ok_or("one-entry has no /entries/0/match")?;
        *entry_match = case.entry_match.clone();
        Ok(())
    })?;
    let entry = plan.entry(0).ok_or("the plan has no entry")?;
    // A walk never answers it: the entry's refusal is matching's layer.
    if entry.refusal().is_some() {
        return Err("an entry its plan's compiling refuses".into());
    }
    let mut budget = build_budget(case.step_limit);
    let verdict = built.read(|environment| answer(entry, environment, &mut budget));
    Ok(encode_verdict(verdict))
}
