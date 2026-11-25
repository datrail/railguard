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
use super::{Outcome, read_cases, read_written, run_each, time};
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
    let summary = |closed: Option<ClosedWindow>| closed.map(|closed| summarize(closed, &names));
    let mut windowing = Windowing::default();
    let mut said = Vec::new();
    for event in &case.events {
        let mut out = match event {
            Event::Hold { plan, at } => {
                json!({"closed": summary(windowing.hold(hold(plan)?, time(*at)))})
            }
            Event::Record { plan, at, call } => {
                let recorded = windowing.record(hold(plan)?, &call.to_call()?, time(*at));
                let outcome = match recorded.outcome {
                    CallOutcome::Counted { body_unread } => {
                        json!({"counted": {"body_unread": body_unread.map(BodyUnreadReason::as_str)}})
                    }
                    CallOutcome::Dropped => json!("dropped"),
                };
                json!({"closed": summary(recorded.closed), "outcome": outcome})
            }
            Event::CountDropped(calls) => {
                windowing.count_dropped(*calls);
                json!({})
            }
        };
        let open_until = windowing
            .end()
            .map(|end| end.duration_since(UNIX_EPOCH).map(|since| since.as_secs()))
            .transpose()
            .map_err(|e| e.to_string())?;
        out["open_until"] = json!(open_until);
        said.push(out);
    }
    Ok(json!(said))
}

// A closed window's plan, span, calls' times and drops, read from its document.
fn summarize(closed: ClosedWindow, names: &BTreeMap<u64, &str>) -> Value {
    let written = serde_json::to_value(closed.into_document(Uuid::nil(), UNIX_EPOCH, 0))
        .expect("a document serializes");
    let seconds = |value: &Value| value.as_str().and_then(read_seconds);
    let version = written["plan_version"].as_u64().unwrap_or_default();
    json!({
        "plan": names.get(&version),
        "start": seconds(&written["window"]["start"]),
        "end": seconds(&written["window"]["end"]),
        "calls_at": written["sequence"].as_array().map(|steps| {
            steps.iter().map(|step| seconds(&step["at"])).collect::<Vec<_>>()
        }),
        "calls_dropped": written["truncated"]["calls_dropped"],
    })
}

// Whole seconds since the epoch, as the vectors write a time.
fn read_seconds(text: &str) -> Option<i64> {
    let read = OffsetDateTime::parse(text, &Rfc3339).ok()?;
    (read.nanosecond() == 0).then(|| read.unix_timestamp())
}
