//! Layer 9: a window's calls, closed into its call record document.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::environment::CallSpec;
use super::plans::{NamedPlans, PlanSpec};
use super::{Outcome, create_outcome, read_cases};
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

pub(super) fn time(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

pub(super) fn run_cases(text: &str, plans: &NamedPlans) -> Result<Vec<Outcome>, String> {
    read_cases::<Case>(text)?
        .into_iter()
        .map(|case| {
            let plan = plans
                .compile(&case.plan, case.entries.as_ref())
                .map_err(|e| format!("{}: {e}", case.id))?;
            let mut window = Window::open(Arc::new(plan), time(case.start));
            window.count_dropped(case.calls_dropped);
            for received in &case.calls {
                // What it says of the call is the windowing's layer.
                let _ = window.record(&received.call.to_call()?, time(received.at));
            }
            let document = window.close(time(case.end)).into_document(
                case.sent.document_id,
                time(case.sent.generated_at),
                case.sent.windows_dropped,
            );
            let got = serde_json::to_value(&document).map_err(|e| e.to_string())?;
            Ok(create_outcome(&case.id, case.expect, got))
        })
        .collect()
}
