//! Layer 3: a path, read for a use, resolved in an environment.

use serde::Deserialize;
use serde_json::{Value, json};

use super::environment::Named;
use super::value::{encode_lookup, encode_refusal};
use super::{Outcome, Use, build_budget, read_cases, run_each};
use crate::capture::read_record_path;
use crate::condition::{Path, resolve_path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(rename = "note")]
    _note: Option<String>,
    environment: String,
    #[serde(rename = "use")]
    use_case: Use,
    path: Value,
    step_limit: Option<u64>,
    expect: Value,
}

pub(super) fn run_cases(text: &str, environments: &Named) -> Result<Vec<Outcome>, String> {
    run_each(read_cases::<Case>(text)?, |case| {
        let got = run_case(&case, environments);
        (case.id, case.expect, got)
    })
}

fn run_case(case: &Case, environments: &Named) -> Result<Value, String> {
    let sources = environments.build(&case.environment)?;
    // Refused as read, its use never resolving it; or as resolved, in the environment.
    let path = match read_path(&case.path, case.use_case) {
        Err(refused) => return Ok(json!({"refused_reading": refused})),
        Ok(path) => path,
    };
    let mut budget = build_budget(case.step_limit);
    Ok(
        match resolve_path(&path, &sources.environment(), &mut budget) {
            Ok((lookup, _)) => encode_lookup(&lookup),
            Err(refusal) => encode_refusal(refusal),
        },
    )
}

// The path as its use reads it, or why it doesn't.
fn read_path(spelled: &Value, use_case: Use) -> Result<Path, &'static str> {
    let path = match spelled {
        Value::String(text) => Path::try_from(text.clone()).ok(),
        Value::Array(steps) => Path::from_steps(steps),
        _ => None,
    }
    .ok_or("invalid")?;
    match use_case.condition_use() {
        Some(use_case) if use_case.reads(&path) => Ok(path),
        Some(_) => Err("not_for_use"),
        // A plan's record reads only a path spelled as a list.
        None => read_record_path(spelled).map_err(|_| "not_for_record"),
    }
}
