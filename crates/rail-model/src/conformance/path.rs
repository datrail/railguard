//! Layer 3: a path, read for a use, resolved in an environment.

use serde::Deserialize;
use serde_json::{Value, json};

use super::environment::Named;
use super::value::{encode_lookup, encode_refusal};
use super::{Outcome, create_outcome, read_cases};
use crate::capture::read_record_path;
use crate::condition::{Budget, ConditionUse, DEFAULT_STEP_LIMIT, Path, Refusal, resolve_path};

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

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Use {
    GatewayPolicy,
    CapturePlan,
    Record,
}

pub(super) fn run_cases(text: &str, environments: &Named) -> Result<Vec<Outcome>, String> {
    read_cases::<Case>(text)?
        .into_iter()
        .map(|case| {
            let sources = environments.build(&case.environment)?;
            let got = match read_path(&case.path, case.use_case) {
                Err(refused) => json!({"refused": refused}),
                Ok(path) => {
                    let mut budget = Budget::new(case.step_limit.unwrap_or(DEFAULT_STEP_LIMIT));
                    match resolve_path(&path, &sources.environment(), &mut budget) {
                        Ok((lookup, _)) => encode_lookup(&lookup),
                        Err(refusal) => encode_refusal(refusal),
                    }
                }
            };
            Ok(create_outcome(&case.id, case.expect, got))
        })
        .collect()
}

// The path as its use reads it, or why it doesn't.
fn read_path(spelled: &Value, use_case: Use) -> Result<Path, &'static str> {
    let path = match spelled {
        Value::String(text) => Path::try_from(text.clone()).ok(),
        Value::Array(steps) => Path::from_steps(steps),
        _ => None,
    }
    .ok_or("invalid")?;
    let reads = match use_case {
        Use::GatewayPolicy => ConditionUse::GatewayPolicy.reads(&path),
        Use::CapturePlan => ConditionUse::CapturePlan.reads(&path),
        // A plan's record reads only a path spelled as a list.
        Use::Record => return read_record_path(spelled).map_err(|_| "not_for_record"),
    };
    if !reads {
        return Err(refusal_name(Refusal::NotForUse));
    }
    Ok(path)
}

fn refusal_name(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::UnknownOp => "unknown_op",
        Refusal::Invalid => "invalid",
        Refusal::NotForUse => "not_for_use",
        Refusal::BudgetExceeded => "budget_exceeded",
    }
}
