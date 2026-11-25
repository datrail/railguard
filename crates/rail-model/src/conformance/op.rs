//! Layer 5: an op and its operand, applied to a value.

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use super::value::{HeldValue, encode_verdict};
use super::{Outcome, create_outcome, read_cases};
use crate::condition::{
    Budget, Condition, ConditionUse, DEFAULT_STEP_LIMIT, apply_condition, compile_condition,
};

// A path every op reads and a gateway policy compiles; never resolved, the value being the case's.
const PATH: &str = "ticket.value";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(rename = "note")]
    _note: Option<String>,
    value: Value,
    op: String,
    // Raw, since some operands are past what a `Value` holds.
    operand: Option<Box<RawValue>>,
    step_limit: Option<u64>,
    expect: Value,
}

pub(super) fn run_cases(text: &str) -> Result<Vec<Outcome>, String> {
    read_cases::<Case>(text)?
        .into_iter()
        .map(|case| {
            let operand = case.operand.as_ref().map_or(String::new(), |operand| {
                format!(r#", "value": {}"#, operand.get())
            });
            let op = Value::from(case.op.as_str());
            let condition = format!(r#"{{"path": "{PATH}", "op": {op}{operand}}}"#);
            let condition: Condition =
                serde_json::from_str(&condition).map_err(|e| format!("{}: {e}", case.id))?;
            let compiled = compile_condition(&condition, ConditionUse::GatewayPolicy);
            let value = HeldValue::decode(&case.value).map_err(|e| format!("{}: {e}", case.id))?;
            let mut budget = Budget::new(case.step_limit.unwrap_or(DEFAULT_STEP_LIMIT));
            let verdict = apply_condition(&compiled, &value.lookup(), &mut budget);
            Ok(create_outcome(
                &case.id,
                case.expect,
                encode_verdict(verdict),
            ))
        })
        .collect()
}
