//! Layer 10: what windowing does with the plan held, calls and drops at chosen times.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::environment::CallSpec;
use super::plans::{NamedPlans, PlanSpec};
use super::window::time;
use super::{Outcome, create_outcome, read_cases};
use crate::capture::{BodyUnreadReason, CallOutcome, ClosedWindow, CompiledPlan, Windowing};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(rename = "note")]
    _note: Option<String>,
    /// Plans named in plans.json, each held as one plan however often it's named; their versions
    /// tell their windows apart.
    plans: Vec<String>,
    events: Vec<Event>,
    expect: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    /// The plan held, or none, at `at`.
    Hold { plan: Option<String>, at: u64 },
    /// A call received at `at`, the plan held then.
    Record {
        plan: Option<String>,
        at: u64,
        call: CallSpec,
    },
    /// Calls lost before reaching the windowing.
    CountDropped(u64),
}

pub(super) fn run_cases(text: &str, plans: &NamedPlans) -> Result<Vec<Outcome>, String> {
    read_cases::<Case>(text)?
        .into_iter()
        .map(|case| {
            run_case(&case, plans)
                .map(|got| create_outcome(&case.id, case.expect.clone(), got))
                .map_err(|e| format!("{}: {e}", case.id))
        })
        .collect()
}

fn run_case(case: &Case, plans: &NamedPlans) -> Result<Value, String> {
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

// Whole seconds since the epoch of `YYYY-MM-DDTHH:MM:SSZ`, as a document writes a whole second.
fn read_seconds(text: &str) -> Option<i64> {
    let field = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    if text.len() != 20 || !text.ends_with('Z') {
        return None;
    }
    let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
    // Days from the civil date, as Howard Hinnant's `days_from_civil`.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}
