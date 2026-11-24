//! A scripted ticket source and a clock on tokio's paused time, for the holder's tests.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_model::agent::AgentId;
use rail_runtime::{
    FetchError, Jitter, Response, TicketHolder, TicketSource, Unavailable, WallClock,
};
use reqwest::StatusCode;
use time::OffsetDateTime;
use tokio::runtime::Handle;
use tokio::time::Instant;

pub const HOUR: Duration = Duration::from_secs(3600);

// A wall clock that moves with tokio's paused time, starting at a fixed instant.
pub struct TestClock {
    pub started: Instant,
}

impl TestClock {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    pub fn wall(&self) -> WallClock {
        let started = self.started;
        Arc::new(move || UNIX_EPOCH + Duration::from_secs(1_800_000_000) + started.elapsed())
    }

    // The wall-clock time `seconds` from the start.
    pub fn at(&self, seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000 + seconds)
    }
}

pub type Answer = Result<Response, FetchError>;

// Answers fetch n with `answers(n)`, `takes` after it starts, recording when it started.
#[derive(Clone)]
pub struct Script {
    pub answers: Arc<dyn Fn(usize) -> Answer + Send + Sync>,
    pub fetched_at: Arc<Mutex<Vec<u64>>>,
    pub agent: AgentId,
    pub started: Instant,
    pub refresh_interval: Duration,
    pub takes: Duration,
}

impl Script {
    pub fn new(
        clock: &TestClock,
        answers: impl Fn(usize) -> Answer + Send + Sync + 'static,
    ) -> Self {
        Self {
            answers: Arc::new(answers),
            fetched_at: Arc::default(),
            agent: AgentId {
                host_id: "host-1".to_owned(),
                sandbox_name: "sandbox a".to_owned(),
            },
            started: clock.started,
            refresh_interval: HOUR,
            takes: Duration::ZERO,
        }
    }

    pub fn fetched_at(&self) -> Vec<u64> {
        self.fetched_at.lock().unwrap().clone()
    }
}

impl TicketSource for Script {
    async fn fetch(&self) -> Answer {
        let fetches = {
            let mut fetched_at = self.fetched_at.lock().unwrap();
            fetched_at.push(self.started.elapsed().as_secs());
            fetched_at.len()
        };
        tokio::time::sleep(self.takes).await;
        (self.answers)(fetches - 1)
    }

    fn agent(&self) -> &AgentId {
        &self.agent
    }

    fn refresh_interval(&self) -> Duration {
        self.refresh_interval
    }
}

pub fn raw(status: StatusCode, body: &str) -> Answer {
    Ok(Response {
        status,
        body: body.as_bytes().to_vec(),
    })
}

// An answer for the script's agent, as Rail Center writes one.
pub fn entry(sandbox: &str, token: &str, expires_at: &str) -> String {
    format!(
        r#"{{"agent_id":"6f1c2e2a-6a5e-4b8e-9d3c-0b6c1f8f2a10","sandbox_name":{sandbox},"agent_key":"default","token":"{token}","expires_at":"{expires_at}"}}"#
    )
}

pub fn envelope(host_id: &str, entries: &[String]) -> String {
    format!(
        r#"{{"host_id":"{host_id}","tickets":[{}]}}"#,
        entries.join(",")
    )
}

pub fn one(token: &str, expires_at: &str) -> String {
    envelope("host-1", &[entry(r#""sandbox a""#, token, expires_at)])
}

// A holder whose wall clock reads `now` throughout, once it has read `answer`.
pub async fn read_at(
    answer: impl Fn() -> Answer + Send + Sync + 'static,
    now: SystemTime,
) -> TicketHolder {
    let clock = TestClock::new();
    let script = Script::new(&clock, move |_| answer());
    let holder = TicketHolder::start(
        &Handle::current(),
        script,
        Arc::new(move || now),
        Jitter::new(|| 1.0),
    );
    until(&clock, 0).await;
    holder
}

// The script's agent's ticket.
pub fn ticket(token: &str, expires_at: SystemTime) -> Answer {
    let at = OffsetDateTime::from(expires_at);
    let expires_at = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    );
    raw(StatusCode::OK, &one(token, &expires_at))
}

pub fn no_ticket() -> Answer {
    raw(StatusCode::OK, &envelope("host-1", &[]))
}

pub fn failed() -> Answer {
    Err(FetchError::Deadline(Duration::from_secs(10)))
}

// With no jitter, so the times the fetches come are exact.
pub fn start(clock: &TestClock, script: &Script, refresh_interval: Duration) -> TicketHolder {
    start_jittered(clock, script, refresh_interval, Jitter::new(|| 1.0))
}

pub fn start_jittered(
    clock: &TestClock,
    script: &Script,
    refresh_interval: Duration,
    jitter: Jitter,
) -> TicketHolder {
    let source = Script {
        refresh_interval,
        ..script.clone()
    };
    TicketHolder::start(&Handle::current(), source, clock.wall(), jitter)
}

pub fn token(holder: &TicketHolder) -> Result<String, Unavailable> {
    holder
        .current()
        .map(|ticket| ticket.token.expose().to_owned())
}

// Lets the refresh loop run up to `seconds` from the start.
pub async fn until(clock: &TestClock, seconds: u64) {
    tokio::time::sleep_until(clock.started + Duration::from_secs(seconds)).await;
    tokio::task::yield_now().await;
}
