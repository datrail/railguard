//! Layer 9: a window's calls, closed into its call record document.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::environment::CallSpec;
use super::plans::{NamedPlans, PlanSpec};
use super::{Outcome, read_cases, run_each, to_time};
use crate::capture::Window;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(rename = "note")]
    _note: Option<String>,
    plan: PlanSpec,
    entries: Option<Value>,
    /// Unix seconds, as every time here.
    start: u64,
    end: u64,
    calls: Vec<Received>,
    calls_dropped: u64,
    sent: Sent,
    expect: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Received {
    at: u64,
    call: CallSpec,
}

/// What the sender gives a document.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sent {
    document_id: Uuid,
    generated_at: u64,
    windows_dropped: u64,
}

pub(super) fn run_cases(text: &str, plans: &NamedPlans) -> Result<Vec<Outcome>, String> {
    run_each(read_cases::<Case>(text)?, |case| {
        let got = run_case(&case, plans);
        (case.id, case.expect, got)
    })
}

fn run_case(case: &Case, plans: &NamedPlans) -> Result<Value, String> {
    let plan = plans.compile(&case.plan, case.entries.as_ref())?;
    let mut window = Window::open(Arc::new(plan), to_time(case.start));
    window.count_dropped(case.calls_dropped);
    for received in &case.calls {
        // What it says of the call is the windowing's layer.
        let _ = window.record(&received.call.to_call()?, to_time(received.at));
    }
    let document = window.close(to_time(case.end)).into_document(
        case.sent.document_id,
        to_time(case.sent.generated_at),
        case.sent.windows_dropped,
    );
    serde_json::to_value(&document).map_err(|e| e.to_string())
}
