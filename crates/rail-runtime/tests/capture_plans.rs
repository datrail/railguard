//! Plans to answer with and the holder started on them, for the capture plan holder's tests.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::time::Duration;

use rail_model::agent::AgentId;
use rail_runtime::{CapturePlanHolder, Jitter};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::runtime::Handle;

pub use crate::scripted::{Answer, Script, answer};

const VECTORS: &str = include_str!("../../../vectors/parse_plan_conformance_cases.json");

pub const MINUTE: Duration = Duration::from_secs(60);

// The agent the vectors' plan names.
pub fn agent() -> AgentId {
    AgentId {
        host_id: "host-7f3c".to_owned(),
        sandbox_name: "agent-finretail".to_owned(),
    }
}

// A content_hash of the form Rail Center writes, told apart by `digit`.
pub fn hash(digit: char) -> String {
    format!("sha256:{}", digit.to_string().repeat(64))
}

// The vectors' plan, valid, with `content_hash` and the root fields in `with` replaced or added.
pub fn plan(content_hash: &str, with: Value) -> Value {
    let vectors: Value = serde_json::from_str(VECTORS).unwrap();
    let mut plan = vectors["plan"].clone();
    plan["content_hash"] = json!(content_hash);
    for (name, value) in with.as_object().cloned().unwrap_or_default() {
        plan[name] = value;
    }
    plan
}

pub fn ok(body: &Value) -> Answer {
    answer(StatusCode::OK, body.to_string())
}

pub fn no_plan() -> Answer {
    answer(StatusCode::NO_CONTENT, b"")
}

// With no jitter, so the times the fetches come are exact.
pub fn start(script: &Script, refresh_interval: Duration) -> CapturePlanHolder {
    start_on(&Handle::current(), script, refresh_interval)
}

pub fn start_on(
    runtime: &Handle,
    script: &Script,
    refresh_interval: Duration,
) -> CapturePlanHolder {
    let plans = Script {
        agent: agent(),
        ..script.clone()
    };
    CapturePlanHolder::start(runtime, plans, refresh_interval, Jitter::new(|| 1.0))
}

// The content_hash of the plan held, if one is.
pub fn held(holder: &CapturePlanHolder) -> Option<String> {
    holder
        .snapshot()
        .map(|plan| plan.plan().content_hash.clone())
}
