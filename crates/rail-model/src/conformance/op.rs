//! Layer 5: an op and its operand, applied to a value.

use serde::{Deserialize, Deserializer};
use serde_json::Value;
use serde_json::value::RawValue;

use super::value::{HeldValue, encode_verdict};
use super::{Outcome, read_cases, run_each};
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
    // Raw, since some operands are past what a `Value` holds; `null` is one, none is no operand.
    #[serde(default, deserialize_with = "read_operand")]
    operand: Option<Box<RawValue>>,
    step_limit: Option<u64>,
    expect: Value,
}

fn read_operand<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(deserializer).map(Some)
}

pub(super) fn run_cases(text: &str) -> Result<Vec<Outcome>, String> {
    run_each(read_cases::<Case>(text)?, |case| {
        let got = run_case(&case);
        (case.id, case.expect, got)
    })
}

fn run_case(case: &Case) -> Result<Value, String> {
    let operand = case.operand.as_ref().map_or(String::new(), |operand| {
        format!(r#", "value": {}"#, operand.get())
    });
    let op = Value::from(case.op.as_str());
    let condition = format!(r#"{{"path": "{PATH}", "op": {op}{operand}}}"#);
    let condition: Condition = serde_json::from_str(&condition).map_err(|e| e.to_string())?;
    let compiled = compile_condition(&condition, ConditionUse::GatewayPolicy);
    let value = HeldValue::decode(&case.value)?;
    let mut budget = Budget::new(case.step_limit.unwrap_or(DEFAULT_STEP_LIMIT));
    Ok(encode_verdict(apply_condition(
        &compiled,
        &value.lookup(),
        &mut budget,
    )))
}
