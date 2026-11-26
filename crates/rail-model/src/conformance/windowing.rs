//! Layer 10: what windowing does with the plan held, calls and drops at chosen times.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use super::environment::CallSpec;
use super::plans::{NamedPlans, PlanSpec};
use super::{Outcome, read_cases, read_written, run_each, to_seconds, to_time};
use crate::capture::{BodyUnreadReason, CallOutcome, ClosedWindow, CompiledPlan, Windowing};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(rename = "note")]
    _note: Option<String>,
    /// The plans its events name, from `capture/plans.json`, each held as one plan however often
    /// it's named; their versions tell their windows apart.
    plans: Vec<String>,
    events: Vec<Event>,
    expect: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    /// The plan held, or none, at `at`.
    Hold {
        #[serde(deserialize_with = "read_written")]
        plan: Option<String>,
        at: u64,
    },
    /// A call received at `at`, the plan held then.
    Record {
        #[serde(deserialize_with = "read_written")]
        plan: Option<String>,
        at: u64,
        call: CallSpec,
    },
    /// Calls lost before reaching the windowing.
    CountDropped(u64),
}

pub(super) fn run_cases(text: &str, plans: &NamedPlans) -> Result<Vec<Outcome>, String> {
    run_each(read_cases::<Case>(text)?, |case| {
        let got = run_case(&case, plans);
        (case.id, case.expect, got)
    })
}

fn run_case(case: &Case, plans: &NamedPlans) -> Result<Value, String> {
    let named: BTreeSet<&str> = (case.events.iter())
        .filter_map(|event| match event {
            Event::Hold { plan, .. } | Event::Record { plan, .. } => plan.as_deref(),
            Event::CountDropped(_) => None,
        })
        .collect();
    if named != case.plans.iter().map(String::as_str).collect() {
        return Err("its plans aren't those its events name".into());
    }
    let mut held: BTreeMap<&str, Arc<CompiledPlan>> = BTreeMap::new();
    let mut names: BTreeMap<u64, &str> = BTreeMap::new();
    for name in &case.plans {
        let plan = plans.compile(&PlanSpec::Named(name.clone()), None)?;
        if names.insert(plan.plan().plan_version, name).is_some() {
            return Err(format!("two plans of version {}", plan.plan().plan_version));
        }
        held.insert(name, Arc::new(plan));
    }
    let hold = |plan: &Option<String>| match plan {
        None => Ok(None),
        Some(name) => held
            .get(name.as_str())
            .map(|plan| Some(Arc::clone(plan)))
            .ok_or_else(|| format!("{name} isn't among the case's plans")),
    };
    let summary =
        |closed: Option<ClosedWindow>| closed.map(|closed| summarize(closed, &names)).transpose();
    let mut windowing = Windowing::default();
    let mut said = Vec::new();
    for event in &case.events {
        let mut out = match event {
            Event::Hold { plan, at } => {
                json!({"closed": summary(windowing.hold(hold(plan)?, to_time(*at)))?})
            }
            Event::Record { plan, at, call } => {
                let recorded = windowing.record(hold(plan)?, &call.to_call()?, to_time(*at));
                let outcome = match recorded.outcome {
                    CallOutcome::Counted { body_unread } => {
                        json!({"counted": {"body_unread": body_unread.map(BodyUnreadReason::as_str)}})
                    }
                    CallOutcome::Dropped => json!("dropped"),
                };
                json!({"closed": summary(recorded.closed)?, "outcome": outcome})
            }
            Event::CountDropped(calls) => {
                windowing.count_dropped(*calls);
                json!({})
            }
        };
        let open_until = windowing.end().map(to_seconds).transpose()?;
        out["open_until"] = json!(open_until);
        said.push(out);
    }
    Ok(json!(said))
}

// A closed window's plan, span, calls' times and drops, read from its document.
fn summarize(closed: ClosedWindow, names: &BTreeMap<u64, &str>) -> Result<Value, String> {
    let written = serde_json::to_value(closed.into_document(Uuid::nil(), UNIX_EPOCH, 0))
        .map_err(|e| e.to_string())?;
    let version = written["plan_version"]
        .as_u64()
        .ok_or("a document with no plan_version")?;
    let plan = names
        .get(&version)
        .ok_or_else(|| format!("a window of no plan named: version {version}"))?;
    let steps = written["sequence"]
        .as_array()
        .ok_or("a document with no sequence")?;
    Ok(json!({
        "plan": plan,
        "start": read_seconds(&written["window"]["start"])?,
        "end": read_seconds(&written["window"]["end"])?,
        "calls_at": steps.iter().map(|step| read_seconds(&step["at"])).collect::<Result<Vec<_>, _>>()?,
        "calls_dropped": written["truncated"]["calls_dropped"],
    }))
}

// Whole seconds since the epoch, as the vectors write a time.
fn read_seconds(written: &Value) -> Result<i64, String> {
    let read = (written.as_str())
        .and_then(|text| OffsetDateTime::parse(text, &Rfc3339).ok())
        .ok_or_else(|| format!("not a time: {written}"))?;
    if read.nanosecond() != 0 {
        return Err(format!("not a whole second: {written}"));
    }
    Ok(read.unix_timestamp())
}
