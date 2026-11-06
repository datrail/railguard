//! A scripted key source, key sets to answer with, and a clock to age them on.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use rail_runtime::{Clock, FetchError, Jitter, KeySetHolder, KeySource, Response};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::runtime::Handle;
use tokio::time::Instant;

use crate::holder;

// The JWT vectors' key.
const X: &str = "A6EHv_POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg";

pub const LIMIT: Duration = Duration::from_secs(2);

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
    pub fn new(
        clock: &holder::Clock,
        answers: impl Fn(usize) -> Answer + Send + Sync + 'static,
    ) -> Self {
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

impl KeySource for Script {
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

pub fn answer(status: StatusCode, body: &[u8]) -> Answer {
    Ok(Response {
        status,
        body: body.to_vec(),
    })
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
    pub fn new(clock: &holder::Clock) -> Self {
        Self {
            started: clock.started,
            skipped: Arc::default(),
        }
    }

    pub fn clock(&self) -> Clock {
        let (started, skipped) = (self.started, Arc::clone(&self.skipped));
        let (boot_started, boot_skipped) = (self.started, Arc::clone(&self.skipped));
        Clock::new(
            move || UNIX_EPOCH + started.elapsed() + *skipped.lock().unwrap(),
            move || boot_started.elapsed() + *boot_skipped.lock().unwrap(),
        )
    }

    pub fn skip(&self, by: Duration) {
        *self.skipped.lock().unwrap() += by;
    }
}

// With no jitter, so the times the fetches come are exact.
pub fn start(clock: &holder::Clock, script: &Script) -> KeySetHolder {
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
