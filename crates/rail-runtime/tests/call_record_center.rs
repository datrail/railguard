//! A Rail Center taking call record documents as a test scripts it, and windows to send it.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rail_model::body::{BodyRead, Unread};
use rail_model::capture::{ClosedWindow, ObservedCall, Window, compile_plan, parse_plan};
use rail_runtime::{CallRecordReporter, CallRecordSink, FetchError, Jitter, Response};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::runtime::Handle;
use tokio::sync::Semaphore;
use tokio::time::Instant;

use crate::capture_plans::{hash, plan};
use crate::holder::TestClock;
use crate::scripted::{Answer, answer};

// When a send started, in whole seconds from the start, and what it sent.
type Sent = (u64, Vec<u8>);

// Rail Center as a test scripts it: answers send n with `answers(n)`, `takes` after it starts and
// once `release` lets it, recording when each started and what it sent.
#[derive(Clone)]
pub struct Center {
    pub answers: Arc<dyn Fn(usize) -> Answer + Send + Sync>,
    pub takes: Duration,
    pub release: Arc<Semaphore>,
    pub sent: Arc<Mutex<Vec<Sent>>>,
    pub started: Instant,
}

impl Center {
    pub fn new(
        clock: &TestClock,
        answers: impl Fn(usize) -> Answer + Send + Sync + 'static,
    ) -> Self {
        Self {
            answers: Arc::new(answers),
            takes: Duration::ZERO,
            release: Arc::new(Semaphore::new(Semaphore::MAX_PERMITS)),
            sent: Arc::default(),
            started: clock.started,
        }
    }

    pub fn accepting(clock: &TestClock) -> Self {
        Self::new(clock, |_| accepted())
    }

    // Holds each send in flight until released.
    pub fn holding(clock: &TestClock) -> Self {
        Self {
            release: Arc::new(Semaphore::new(0)),
            ..Self::accepting(clock)
        }
    }

    pub fn start(&self, clock: &TestClock) -> CallRecordReporter {
        CallRecordReporter::start(
            &Handle::current(),
            self.clone(),
            clock.wall(),
            Jitter::new(|| 1.0),
        )
    }

    // When each send started, in whole seconds from the start.
    pub fn sent_at(&self) -> Vec<u64> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|(at, _)| *at)
            .collect()
    }

    pub fn bodies(&self) -> Vec<Vec<u8>> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|(_, body)| body.clone())
            .collect()
    }

    pub fn documents(&self) -> Vec<Value> {
        self.bodies()
            .iter()
            .map(|body| serde_json::from_slice(body).unwrap())
            .collect()
    }
}

impl CallRecordSink for Center {
    async fn send(&self, body: Vec<u8>) -> Result<Response, FetchError> {
        let sends = {
            let mut sent = self.sent.lock().unwrap();
            sent.push((self.started.elapsed().as_secs(), body));
            sent.len()
        };
        tokio::time::sleep(self.takes).await;
        self.release.acquire().await.unwrap().forget();
        (self.answers)(sends - 1)
    }
}

pub fn accepted() -> Answer {
    answer(StatusCode::ACCEPTED, b"")
}

// A window closed under the vectors' plan, `max_unsent_windows` replaced, holding a call to each
// of `paths`.
pub fn window(max_unsent_windows: u32, paths: &[&str]) -> ClosedWindow {
    let mut plan = plan(&hash('a'), json!({}));
    plan["budget"]["max_unsent_windows"] = json!(max_unsent_windows);
    let plan = Arc::new(compile_plan(parse_plan(&plan.to_string()).unwrap()));
    let clock = TestClock::new();
    let mut window = Window::open(plan, clock.at(0));
    for path in paths {
        let call = ObservedCall {
            authority: b"api.github.com".to_vec(),
            method: b"GET".to_vec(),
            path: path.as_bytes().to_vec(),
            body: BodyRead::Unread(Unread::NotSent),
            body_bytes: 0,
        };
        window.record(&call, clock.at(1));
    }
    window.close(clock.at(2))
}

pub fn calls(document: &Value) -> u64 {
    document["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|host| host["count"].as_u64().unwrap())
        .sum()
}

pub fn dropped(document: &Value) -> u64 {
    document["truncated"]["windows_dropped"].as_u64().unwrap()
}
