//! Layer 7: what a record's keys and measures read of a call.

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::environment::Named;
use super::plans::{NamedPlans, compile_written};
use super::value::{
    encode_body_unread, encode_host_reading, encode_refusal, encode_reported_method,
};
use super::{Outcome, read_cases, run_each};
use crate::capture::{
    KeptValue, KeyReading, MeasureReading, PathReading, Record, RecordReading, Replacement,
    read_record,
};
use crate::condition::{Budget, DEFAULT_STEP_LIMIT};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(rename = "note")]
    _note: Option<String>,
    environment: String,
    record_of: RecordOf,
    record: Value,
    step_limit: Option<u64>,
    expect: Value,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RecordOf {
    Entry,
    CatchAll,
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
    let sources = environments.build(&case.environment)?;
    let (call, request_line) = sources.call()?;
    let mut plan = plans.get("one-entry")?;
    match case.record_of {
        RecordOf::Entry => plan["entries"][0]["record"] = case.record.clone(),
        RecordOf::CatchAll => plan["catch_all"]["record"] = case.record.clone(),
    }
    let plan = compile_written(&plan)?;
    let record = match case.record_of {
        RecordOf::Entry => &plan.plan().entries[0].record,
        RecordOf::CatchAll => &plan.plan().catch_all.record,
    };
    let mut budget = Budget::new(case.step_limit.unwrap_or(DEFAULT_STEP_LIMIT));
    let reading = read_record(
        record,
        request_line,
        &sources.environment(),
        call.body_bytes,
        &mut budget,
    );
    Ok(encode_record_reading(record, &reading))
}

// Each key's and measure's reading by its name, which a record holds once.
fn encode_record_reading(record: &Record, reading: &RecordReading) -> Value {
    let keys: Map<String, Value> = record
        .context_keys
        .iter()
        .zip(&reading.context_keys)
        .map(|(key, reading)| (key.name().to_owned(), encode_key_reading(reading)))
        .collect();
    let measures: Map<String, Value> = record
        .measures
        .iter()
        .zip(&reading.measures)
        .map(|(measure, reading)| (measure.name().to_owned(), encode_measure_reading(reading)))
        .collect();
    json!({"context_keys": keys, "measures": measures})
}

fn encode_key_reading(reading: &KeyReading) -> Value {
    match reading {
        KeyReading::Method(method) => json!({"method": encode_reported_method(*method)}),
        KeyReading::Host(host) => json!({"host": encode_host_reading(host)}),
        KeyReading::PathPrefix(prefix) => json!({"path_prefix": prefix}),
        KeyReading::Operation(reading) => {
            json!({"operation": encode_path_reading(reading, encode_kept)})
        }
        KeyReading::Path(reading) => json!({"path": encode_path_reading(reading, encode_kept)}),
    }
}

fn encode_measure_reading(reading: &MeasureReading) -> Value {
    match reading {
        MeasureReading::CallCount => json!("call_count"),
        MeasureReading::ErrorCount => json!("error_count"),
        MeasureReading::RequestBytes(bytes) => json!({"request_bytes": bytes}),
        MeasureReading::SizeInBytes(readings) => json!({"size_in_bytes": readings
            .iter()
            .map(|reading| encode_path_reading(reading, |bytes| json!({"bytes": bytes})))
            .collect::<Vec<_>>()}),
    }
}

fn encode_path_reading<T>(reading: &PathReading<T>, found: impl Fn(&T) -> Value) -> Value {
    match reading {
        PathReading::Found(value) => json!({"found": found(value)}),
        PathReading::Absent => json!("absent"),
        PathReading::Ambiguous => json!("ambiguous"),
        PathReading::BodyUnread(why) => json!({"body_unread": encode_body_unread(*why)}),
        PathReading::Refused(refusal) => encode_refusal(*refusal),
    }
}

fn encode_kept(value: &KeptValue) -> Value {
    match value {
        KeptValue::String(text) => json!({"string": text}),
        KeptValue::Hashed(hex) => json!({"hashed": hex}),
        KeptValue::Replaced(Replacement::KeyShape) => json!({"replaced": "key_shape"}),
        KeptValue::Replaced(Replacement::PastLongest) => json!({"replaced": "past_longest"}),
        KeptValue::NotAString => json!("not_a_string"),
    }
}
