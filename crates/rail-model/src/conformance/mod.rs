//! The conformance vectors' runner: each layer's cases read, run, and what they find written in the
//! vectors' encoding, so the Rust tests and the binding compare the same values. Never in a
//! production build.

use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Number, Value};

mod condition;
mod entry;
mod environment;
mod matching;
mod mcp;
mod op;
mod path;
mod plan;
mod plans;
mod record;
mod request_line;
mod value;
mod window;
mod windowing;

/// One case run: what its `expect` says, and what this reader found.
#[derive(Debug)]
pub struct Outcome {
    pub id: String,
    pub expect: Value,
    pub got: Value,
}

/// Every case of `file`, a path under `vectors`, run by its layer.
pub fn run_vectors(vectors: &Path, file: &str) -> Result<Vec<Outcome>, String> {
    let read =
        |name: &str| fs::read_to_string(vectors.join(name)).map_err(|e| format!("{name}: {e}"));
    let cases = read(file)?;
    let read_plans = || plans::NamedPlans::read(&read("capture/plans.json")?);
    let read_environments = || environment::Named::read(&read("environments.json")?);
    let outcomes = match file {
        "request_line.json" => request_line::run_cases(&cases),
        "mcp.json" => mcp::run_cases(&cases),
        "capture/matching.json" => matching::run_cases(&cases, &read_plans()?),
        "capture/windowing.json" => windowing::run_cases(&cases, &read_plans()?),
        "capture/window.json" => window::run_cases(&cases, &read_plans()?),
        "capture/entry.json" => entry::run_cases(&cases, &read_environments()?),
        "capture/record.json" => record::run_cases(&cases, &read_environments()?),
        "condition/condition.json" => condition::run_cases(&cases),
        "condition/op.json" => op::run_cases(&cases),
        "condition/path.json" => path::run_cases(&cases, &read_environments()?),
        _ => return Err(format!("{file}: no layer reads it")),
    };
    outcomes.map_err(|e| format!("{file}: {e}"))
}

// A file's cases, the fields every vector file may have beside them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases<C> {
    #[serde(rename = "_source")]
    _source: Option<Value>,
    cases: Vec<C>,
}

fn read_cases<C: DeserializeOwned>(text: &str) -> Result<Vec<C>, String> {
    serde_json::from_str::<Cases<C>>(text)
        .map(|file| file.cases)
        .map_err(|e| e.to_string())
}

fn create_outcome(id: &str, expect: Value, got: Value) -> Outcome {
    Outcome {
        id: id.to_owned(),
        expect: normalize_numbers(expect),
        got: normalize_numbers(got),
    }
}

// A whole number written either way compares equal: `1` and `1.0`.
fn normalize_numbers(value: Value) -> Value {
    match value {
        Value::Number(number) => match number.as_f64() {
            Some(float)
                if number.is_f64() && float.fract() == 0.0 && float.abs() < 2f64.powi(53) =>
            {
                Value::Number(Number::from(float as i64))
            }
            _ => Value::Number(number),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_numbers).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, normalize_numbers(value)))
                .collect(),
        ),
        other => other,
    }
}
