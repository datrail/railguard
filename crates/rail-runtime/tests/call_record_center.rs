//! A Rail Center taking call record documents as a test scripts it, and windows to send it.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use rail_model::body::{BodyRead, Unread};
use rail_model::capture::{ClosedWindow, ObservedCall, Window, compile_plan, parse_plan};
use rail_runtime::{CallRecordReporter, CallRecordSink, FetchError, Jitter};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::runtime::Handle;
use tokio::sync::Semaphore;

use crate::capture_plans::{hash, plan};
use crate::holder::TestClock;
use crate::scripted::{Answer, Script, answer};

// Rail Center as a test scripts it: answers send n with its script's nth answer, once `release`
// lets it, keeping what each sent.
#[derive(Clone)]
pub struct Center {
    pub script: Script,
    pub release: Arc<Semaphore>,
    pub bodies: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Center {
    pub fn new(
        clock: &TestClock,
        answers: impl Fn(usize) -> Answer + Send + Sync + 'static,
    ) -> Self {
        Self {
            script: Script::new(clock, answers),
            release: Arc::new(Semaphore::new(Semaphore::MAX_PERMITS)),
            bodies: Arc::default(),
        }
    }

    pub fn accepting(clock: &TestClock) -> Self {
        Self::new(clock, |_| accepted())
    }

    // Holds each send in flight until released.
    pub fn holding(self) -> Self {
        Self {
            release: Arc::new(Semaphore::new(0)),
            ..self
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
        self.script.fetched_at()
    }

    pub fn bodies(&self) -> Vec<Vec<u8>> {
        self.bodies.lock().unwrap().clone()
    }

    pub fn documents(&self) -> Vec<Value> {
        self.bodies()
            .iter()
            .map(|body| serde_json::from_slice(body).unwrap())
            .collect()
    }
}

impl CallRecordSink for Center {
    async fn send(&self, body: Vec<u8>) -> Result<StatusCode, FetchError> {
        self.bodies.lock().unwrap().push(body);
        let answer = self.script.answer().await;
        self.release.acquire().await.unwrap().forget();
        answer.map(|response| response.status)
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

// Each document's calls and the drops it carries.
pub fn windows(center: &Center) -> Vec<(u64, u64)> {
    center
        .documents()
        .iter()
        .map(|document| (calls(document), dropped(document)))
        .collect()
}

pub fn dropped(document: &Value) -> u64 {
    document["truncated"]["windows_dropped"].as_u64().unwrap()
}
