//! The conformance vectors' runner: each layer's cases read, run, and what they find written in the
//! vectors' encoding, so the Rust tests and the binding compare the same values. In the binding's
//! wheel, never in the gateway's or the proxy's binaries. It reads only the texts it's given.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::condition::{Budget, ConditionUse, DEFAULT_STEP_LIMIT};

mod condition;
mod entry;
mod environment;
mod matching;
mod mcp;
mod op;
mod path;
mod plans;
mod record;
mod request_line;
mod value;
mod window;
mod windowing;

/// Every file of cases the runner reads, by its path under `vectors/`.
pub const FILES: [&str; 10] = [
    "request_line.json",
    "mcp.json",
    "condition/path.json",
    "condition/condition.json",
    "condition/op.json",
    "capture/entry.json",
    "capture/record.json",
    "capture/matching.json",
    "capture/window.json",
    "capture/windowing.json",
];

/// The files cases name inputs from.
pub const SHARED: [&str; 2] = ["environments.json", "capture/plans.json"];

/// One case run: what its `expect` says, and what this reader found.
#[derive(Debug)]
pub struct Outcome {
    pub id: String,
    pub expect: Value,
    pub got: Value,
}

/// Every case of `file`, one of `FILES`, run by its layer. `texts` holds, by path under `vectors/`,
/// `file` and each of `SHARED`.
pub fn run_vectors(file: &str, texts: &BTreeMap<String, String>) -> Result<Vec<Outcome>, String> {
    let read = |name: &str| {
        texts
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("{name} not given"))
    };
    let cases = read(file)?;
    let plans = || plans::NamedPlans::read(read("capture/plans.json")?);
    let environments = || environment::Named::read(read("environments.json")?);
    let outcomes = match file {
        "request_line.json" => request_line::run_cases(cases),
        "mcp.json" => mcp::run_cases(cases),
        "condition/path.json" => path::run_cases(cases, &environments()?),
        "condition/condition.json" => condition::run_cases(cases),
        "condition/op.json" => op::run_cases(cases),
        "capture/entry.json" => entry::run_cases(cases, &environments()?, &plans()?),
        "capture/record.json" => record::run_cases(cases, &environments()?, &plans()?),
        "capture/matching.json" => matching::run_cases(cases, &plans()?),
        "capture/window.json" => window::run_cases(cases, &plans()?),
        "capture/windowing.json" => windowing::run_cases(cases, &plans()?),
        _ => return Err(format!("{file}: no layer reads it")),
    };
    outcomes.map_err(|e| format!("{file}: {e}"))
}

// A file's cases.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases<C> {
    cases: Vec<C>,
}

fn read_cases<C: DeserializeOwned>(text: &str) -> Result<Vec<C>, String> {
    serde_json::from_str::<Cases<C>>(text)
        .map(|file| file.cases)
        .map_err(|e| e.to_string())
}

// Each case its outcome, from its id, its `expect` and what running it found; a case that can't
// run fails the file, naming the case.
fn run_each<C>(
    cases: Vec<C>,
    run: impl Fn(C) -> (String, Value, Result<Value, String>),
) -> Result<Vec<Outcome>, String> {
    cases
        .into_iter()
        .map(|case| {
            let (id, expect, got) = run(case);
            let got = got.map_err(|e| format!("{id}: {e}"))?;
            Ok(Outcome { id, expect, got })
        })
        .collect()
}

// Unix seconds, as every time in the vectors.
fn to_time(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn to_seconds(time: SystemTime) -> Result<u64, String> {
    time.duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .map_err(|e| e.to_string())
}

// Past any case's body and tree, but finite, so a parser that never stops runs out of it.
const DEFAULT_BYTE_BUDGET: usize = 32 << 20;

// A case's step limit, the default where it gives none.
fn build_budget(step_limit: Option<u64>) -> Budget {
    Budget::new(step_limit.unwrap_or(DEFAULT_STEP_LIMIT))
}

// An `Option` the case must write, `null` being one of its values.
fn read_written<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(deserializer)
}

// Which use a path or a condition is read for: a record's, or a condition's.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Use {
    GatewayPolicy,
    CapturePlan,
    Record,
}

impl Use {
    fn condition_use(self) -> Option<ConditionUse> {
        match self {
            Self::GatewayPolicy => Some(ConditionUse::GatewayPolicy),
            Self::CapturePlan => Some(ConditionUse::CapturePlan),
            Self::Record => None,
        }
    }
}
