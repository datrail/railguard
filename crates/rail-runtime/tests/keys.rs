//! A scripted key source, key sets to answer with, and a clock to age them on.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use rail_runtime::{Clock, FetchError, Jitter, KeySetHolder};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::runtime::Handle;
use tokio::time::Instant;

use crate::holder::TestClock;
pub use crate::scripted::{Answer, Script, answer};

// The JWT vectors' key.
const X: &str = "A6EHv_POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg";

pub const LIMIT: Duration = Duration::from_secs(2);

// A set of one key under each of `kids`.
pub fn key_set(kids: &[&str]) -> Value {
    let keys: Vec<Value> = kids
        .iter()
        .map(|kid| json!({"kty": "OKP", "crv": "Ed25519", "kid": kid, "x": X}))
        .collect();
    json!({ "keys": keys })
}

pub fn ok(kids: &[&str]) -> Answer {
    answer(StatusCode::OK, key_set(kids).to_string().as_bytes())
}

pub fn failed() -> Answer {
    Err(FetchError::Deadline(Duration::from_secs(10)))
}

// Both readings move with tokio's paused time, and `skip` moves them alone, as a suspended host.
#[derive(Clone)]
pub struct AgingClock {
    started: Instant,
    skipped: Arc<Mutex<Duration>>,
}

impl AgingClock {
    pub fn new(clock: &TestClock) -> Self {
        Self {
            started: clock.started,
            skipped: Arc::default(),
        }
    }

    pub fn clock(&self) -> Clock {
        let (wall, boot) = (self.clone(), self.clone());
        Clock::new(move || UNIX_EPOCH + wall.elapsed(), move || boot.elapsed())
    }

    fn elapsed(&self) -> Duration {
        self.started.elapsed() + *self.skipped.lock().unwrap()
    }

    pub fn skip(&self, by: Duration) {
        *self.skipped.lock().unwrap() += by;
    }
}

// With no jitter, so the times the fetches come are exact.
pub fn start(clock: &TestClock, script: &Script) -> KeySetHolder {
    start_aging(&AgingClock::new(clock), script)
}

pub fn start_aging(aging: &AgingClock, script: &Script) -> KeySetHolder {
    KeySetHolder::start(
        &Handle::current(),
        script.clone(),
        aging.clock(),
        Jitter::new(|| 1.0),
    )
}

// The kids of the set held and still fresh, sorted.
pub fn kids(holder: &KeySetHolder) -> Option<Vec<String>> {
    let mut kids: Vec<String> = holder.keys()?.keys.keys().cloned().collect();
    kids.sort();
    Some(kids)
}

pub fn named(kids: &[&str]) -> Option<Vec<String>> {
    Some(kids.iter().map(|kid| (*kid).to_owned()).collect())
}
