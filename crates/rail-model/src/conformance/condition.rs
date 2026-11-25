//! Layer 4: a condition as sent, read, compiled for a use, and checked as Rail Center checks it.

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use super::value::encode_refusal;
use super::{Outcome, create_outcome};
use crate::condition::{
    Condition, ConditionUse, TicketClaimsSchema, check_condition, compile_condition,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases {
    #[serde(rename = "_source")]
    _source: Option<Value>,
    ticket_claims_schema: Value,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(rename = "note")]
    _note: Option<String>,
    // Raw, since some are past what a `Value` holds.
    condition: Box<RawValue>,
    #[serde(rename = "use")]
    use_case: Use,
    expect: Value,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Use {
    GatewayPolicy,
    CapturePlan,
}

pub(super) fn run_cases(text: &str) -> Result<Vec<Outcome>, String> {
    let file: Cases = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let schema = TicketClaimsSchema::from_value(&file.ticket_claims_schema)?;
    file.cases
        .into_iter()
        .map(|case| {
            let use_case = match case.use_case {
                Use::GatewayPolicy => ConditionUse::GatewayPolicy,
                Use::CapturePlan => ConditionUse::CapturePlan,
            };
            let text = case.condition.get();
            let condition: Condition =
                serde_json::from_str(text).map_err(|e| format!("{}: {e}", case.id))?;
            let reading = match &condition {
                Condition::Valid(_) => "valid",
                Condition::Unknown(_) => "unknown",
                Condition::Invalid(_) => "invalid",
            };
            let compiled = match compile_condition(&condition, use_case).refusal() {
                None => json!("evaluable"),
                Some(refusal) => encode_refusal(refusal),
            };
            let problems: Vec<Value> = check_condition(text, use_case, &schema)
                .into_iter()
                .map(|problem| {
                    let mut written = json!({
                        "location": problem.location,
                        "code": problem.code,
                        "scope": problem.scope,
                    });
                    // Its wording is this reader's own, so not a vector's.
                    if let Some(shape) = problem.expected_shape {
                        written["expected_shape"] = json!(shape);
                    }
                    written
                })
                .collect();
            let got = json!({"reading": reading, "compiled": compiled, "problems": problems});
            Ok(create_outcome(&case.id, case.expect, got))
        })
        .collect()
}
