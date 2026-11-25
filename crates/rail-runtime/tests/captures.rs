//! Calls to hand over and capture started on scripted plans, for the capture task's tests.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use rail_model::body::{BodyRead, Unread};
use rail_model::capture::ObservedCall;
use rail_runtime::{Capture, CaptureSender};
use serde_json::json;
use tokio::runtime::Handle;

use crate::call_record_center::Center;
use crate::capture_plans::{MINUTE, hash, ok, plan, start};
use crate::holder::TestClock;
use crate::scripted::{Answer, Script};

pub fn call(path: &str, body: BodyRead) -> ObservedCall {
    ObservedCall {
        authority: b"api.github.com".to_vec(),
        method: b"POST".to_vec(),
        path: path.as_bytes().to_vec(),
        body,
        body_bytes: 0,
    }
}

pub fn get() -> ObservedCall {
    call("/a", BodyRead::Unread(Unread::NotSent))
}

// Capture under the plans `answers` gives, refetched every minute, reporting to `center`.
pub fn capture(
    clock: &TestClock,
    answers: impl Fn(usize) -> Answer + Send + Sync + 'static,
    center: &Center,
) -> (Capture, CaptureSender) {
    let plans = start(&Script::new(clock, answers), MINUTE);
    Capture::start(&Handle::current(), plans, center.start(clock), clock.wall())
}

// The vectors' plan, with 900 s windows.
pub fn held() -> Answer {
    ok(&plan(&hash('a'), json!({})))
}
