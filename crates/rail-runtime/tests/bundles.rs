//! A scripted bundle source and bundles to answer with, for the bundle holder's tests.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rail_runtime::{BundleHolder, BundleSource, FetchError, Jitter, Response};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::runtime::Handle;
use tokio::time::Instant;

use crate::holder::Clock;

pub const ONE: &str = "5c8f1e42-0000-4000-8000-0000000000a1";
pub const TWO: &str = "5c8f1e42-0000-4000-8000-0000000000a2";

pub const MINUTE: Duration = Duration::from_secs(60);

pub type Answer = Result<Response, FetchError>;

// Answers fetch n with `answers(n)`, `takes` after it starts, recording when it started.
#[derive(Clone)]
pub struct Script {
    pub answers: Arc<dyn Fn(usize) -> Answer + Send + Sync>,
    pub takes: Duration,
    pub fetched_at: Arc<Mutex<Vec<u64>>>,
    pub started: Instant,
}

impl Script {
    pub fn new(clock: &Clock, answers: impl Fn(usize) -> Answer + Send + Sync + 'static) -> Self {
        Self {
            answers: Arc::new(answers),
            takes: Duration::ZERO,
            fetched_at: Arc::default(),
            started: clock.started,
        }
    }

    // When each fetch started, in whole seconds from the start.
    pub fn fetched_at(&self) -> Vec<u64> {
        self.fetched_at.lock().unwrap().clone()
    }
}

impl BundleSource for Script {
    async fn fetch(&self) -> Answer {
        let fetches = {
            let mut fetched_at = self.fetched_at.lock().unwrap();
            fetched_at.push(self.started.elapsed().as_secs());
            fetched_at.len()
        };
        tokio::time::sleep(self.takes).await;
        (self.answers)(fetches - 1)
    }
}

// A usable bundle, observing; `with` replaces or adds root fields.
pub fn bundle(content_hash: &str, with: Value) -> Value {
    let mut bundle = json!({
        "schema_version": "2.0",
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
    });
    for (name, value) in with.as_object().cloned().unwrap_or_default() {
        bundle[name] = value;
    }
    bundle
}

pub fn ok(body: &Value) -> Answer {
    answer(StatusCode::OK, body.to_string().as_bytes())
}

pub fn answer(status: StatusCode, body: &[u8]) -> Answer {
    Ok(Response {
        status,
        body: body.to_vec(),
    })
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
