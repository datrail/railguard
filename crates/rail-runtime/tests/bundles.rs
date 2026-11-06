//! A scripted bundle source and bundles to answer with, for the bundle holder's tests.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::time::Duration;

use rail_runtime::{BundleHolder, FetchError, Jitter};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::runtime::Handle;

pub use crate::scripted::{Answer, Script, answer};

pub const ONE: &str = "5c8f1e42-0000-4000-8000-0000000000a1";
pub const TWO: &str = "5c8f1e42-0000-4000-8000-0000000000a2";

pub const MINUTE: Duration = Duration::from_secs(60);

// A usable bundle, observing; `with` replaces or adds root fields.
pub fn bundle(content_hash: &str, with: Value) -> Value {
    let mut bundle = json!({
        "schema_version": "2.1",
        "content_hash": content_hash,
        "generated_at": "2026-07-28T20:41:07.512247Z",
        "enforcement": {"mode": "observe"},
        "policies": [{
            "id": ONE,
            "name": "P",
            "priority": 1,
            "condition": {"path": "ticket.posture.overall.score", "op": "lt", "value": 40},
            "action": "block",
            "enabled": true,
        }],
        "bindings": [],
        "binding_fallback": "block",
        "ticket_fallback": "pass",
        // As while Rail Center signs no tickets.
        "ticket_issuer": null,
        "ticket_audience": null,
    });
    for (name, value) in with.as_object().cloned().unwrap_or_default() {
        bundle[name] = value;
    }
    bundle
}

pub fn ok(body: &Value) -> Answer {
    answer(StatusCode::OK, body.to_string().as_bytes())
}

pub fn failed() -> Answer {
    Err(FetchError::Deadline(Duration::from_secs(30)))
}

// With no jitter, so the times the fetches come are exact.
pub fn start(script: &Script, refresh_interval: Duration) -> BundleHolder {
    BundleHolder::start(
        &Handle::current(),
        script.clone(),
        refresh_interval,
        Jitter::new(|| 1.0),
    )
}

pub fn held_hash(holder: &BundleHolder) -> Option<String> {
    holder.snapshot().map(|bundle| bundle.content_hash.clone())
}
