//! A denial sink the reporter's tests hold reports in, and the denials they report.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_policy::{Endpoint, Ticket, TicketState};
use rail_runtime::{Denial, DenialReporter, DenialSink, DeniedBy, FetchError, Response};
use reqwest::StatusCode;
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;
use uuid::Uuid;

pub const POLICY: Uuid = Uuid::from_u128(0x11111111_0000_4000_8000_000000000001);

// 2026-11-06T12:00:00.512247Z.
pub fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_793_966_400) + Duration::from_micros(512_247)
}

static RESOLVED: LazyLock<Endpoint> =
    LazyLock::new(|| Endpoint::Resolved("/mcp#tools/call#getPrice".parse().unwrap()));

// A denial of a call to a resolved endpoint with no ticket.
pub fn denial(denied_by: DeniedBy) -> Denial<'static> {
    Denial {
        denied_by,
        endpoint: &RESOLVED,
        ticket: &TicketState::Absent,
        claimed_status: None,
    }
}

pub fn valid(claims: Value) -> TicketState {
    let Value::Object(claims) = claims else {
        panic!("{claims}")
    };
    TicketState::Valid(Box::new(Ticket {
        exp_epoch_seconds: 1_893_456_000,
        claims,
    }))
}

// Records each report, and answers once `release` lets it.
#[derive(Clone)]
pub struct Sink {
    pub reports: Arc<Mutex<Vec<Value>>>,
    pub release: Arc<Semaphore>,
    pub answer: StatusCode,
}

impl Sink {
    // Answers 202 at once.
    pub fn accepting() -> Self {
        Self::answering(StatusCode::ACCEPTED)
    }

    pub fn answering(answer: StatusCode) -> Self {
        Self {
            reports: Arc::default(),
            release: Arc::new(Semaphore::new(Semaphore::MAX_PERMITS)),
            answer,
        }
    }

    // Holds each report in flight until released.
    pub fn holding() -> Self {
        Self {
            release: Arc::new(Semaphore::new(0)),
            ..Self::accepting()
        }
    }

    pub fn reports(&self) -> Vec<Value> {
        self.reports.lock().unwrap().clone()
    }

    pub fn start(&self) -> DenialReporter {
        DenialReporter::new(&Handle::current(), self.clone(), Arc::new(now))
    }

    // Lets the reporter's tasks run until `count` reports have reached the sink.
    pub async fn until_sent(&self, count: usize) {
        for _ in 0..1000 {
            if self.reports.lock().unwrap().len() >= count {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("{} reports sent, not {count}", self.reports().len());
    }
}

impl DenialSink for Sink {
    async fn send(&self, body: Vec<u8>) -> Result<Response, FetchError> {
        self.reports
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&body).unwrap());
        self.release.acquire().await.unwrap().forget();
        Ok(Response {
            status: self.answer,
            body: Vec::new(),
        })
    }
}
