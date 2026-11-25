//! Layer 8: a plan compiled, and the record a call is taken by, and why.

use serde::Deserialize;
use serde_json::{Value, json};

use super::environment::CallSpec;
use super::plans::{NamedPlans, PlanSpec};
use super::value::{encode_body_unread, encode_unmatchable};
use super::{Outcome, read_cases, run_each};
use crate::capture::{CompiledPlan, EntryRefusal, Taken, Untaken};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(rename = "note")]
    _note: Option<String>,
    plan: PlanSpec,
    entries: Option<Value>,
    call: CallSpec,
    expect: Value,
}

pub(super) fn run_cases(text: &str, plans: &NamedPlans) -> Result<Vec<Outcome>, String> {
    run_each(read_cases::<Case>(text)?, |case| {
        let got = run_case(&case, plans);
        (case.id, case.expect, got)
    })
}

fn run_case(case: &Case, plans: &NamedPlans) -> Result<Value, String> {
    let plan = plans.compile(&case.plan, case.entries.as_ref())?;
    let captured = plan.capture(&case.call.to_call()?);
    Ok(json!({
        "refused": encode_refused(&plan),
        "entries_skipped": plan.entries_skipped(),
        "taken": encode_taken(&captured.taken),
    }))
}

fn encode_refused(plan: &CompiledPlan) -> Vec<Value> {
    plan.refused()
        .filter_map(|entry| Some((entry.id(), entry.refusal()?)))
        .map(|(id, refusal)| encode_entry_refusal(id, refusal))
        .collect()
}

fn encode_entry_refusal(id: &str, refusal: &EntryRefusal) -> Value {
    json!({"entry": id, "location": refusal.location, "refusal": refusal.refusal})
}

fn encode_taken(taken: &Taken) -> Value {
    match taken {
        Taken::Entry(entry) => json!({"entry": entry.id}),
        Taken::CatchAll(catch_all, why) => {
            json!({"catch_all": catch_all.id, "why": encode_untaken(why)})
        }
    }
}

fn encode_untaken(why: &Untaken) -> Value {
    match why {
        Untaken::NoEntry => json!("no_entry"),
        Untaken::Unmatchable(why) => json!({"unmatchable": encode_unmatchable(*why)}),
        Untaken::Refused(entry, refusal) => {
            json!({"refused": encode_entry_refusal(&entry.id, refusal)})
        }
        Untaken::Stopped(entry, refusal) => {
            json!({"stopped": {"entry": entry.id, "refusal": refusal}})
        }
        Untaken::Ambiguous(entry) => json!({"ambiguous": {"entry": entry.id}}),
        Untaken::BodyUnread(entry, why) => {
            json!({"body_unread": {"entry": entry.id, "why": encode_body_unread(*why)}})
        }
    }
}
