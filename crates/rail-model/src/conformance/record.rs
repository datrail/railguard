//! Layer 7: what a record's keys and measures read of a call.

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::environment::Named;
use super::plans::NamedPlans;
use super::value::{
    encode_body_unread, encode_host_reading, encode_refusal, encode_reported_method,
};
use super::{Outcome, build_budget, read_cases, run_each};
use crate::capture::{
    KeptValue, KeyReading, MeasureReading, PathReading, Plan, Record, RecordReading, Replacement,
    read_record,
};

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
    let built = environments.build(&case.environment)?;
    let call = built.call()?;
    let (pointer, written): (&str, fn(&Plan) -> &Record) = match case.record_of {
        RecordOf::Entry => ("/entries/0/record", |plan| &plan.entries[0].record),
        RecordOf::CatchAll => ("/catch_all/record", |plan| &plan.catch_all.record),
    };
    let plan = plans.compile_edited("one-entry", |plan| {
        let record = plan
            .pointer_mut(pointer)
            .ok_or_else(|| format!("one-entry has no {pointer}"))?;
        *record = case.record.clone();
        Ok(())
    })?;
    let record = written(plan.plan());
    let mut budget = build_budget(case.step_limit);
    let reading = read_record(record, call, &mut budget);
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
        .map(|(measure, reading)| (measure.name.clone(), encode_measure_reading(reading)))
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
        MeasureReading::SizeInBytes(readings) => json!({"size_in_bytes": readings
            .iter()
            .map(|reading| encode_path_reading(reading, |bytes| json!({"bytes": bytes})))
            .collect::<Vec<_>>()}),
        MeasureReading::IsSet(readings) => json!({"is_set": readings
            .iter()
            .map(|reading| encode_path_reading(reading, |set| json!({"set": set})))
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
