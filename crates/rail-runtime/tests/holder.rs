//! A scripted ticket source and a clock on tokio's paused time, for the holder's tests.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_runtime::{
    Jitter, OpaqueTicket, Secret, TicketAnswer, TicketHolder, TicketSource, Unavailable, WallClock,
};
use tokio::runtime::Handle;
use tokio::time::Instant;

pub const HOUR: Duration = Duration::from_secs(3600);

// A wall clock that moves with tokio's paused time, starting at a fixed instant.
pub struct Clock {
    pub started: Instant,
}

impl Clock {
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

pub type Answer = Result<TicketAnswer, String>;

// Answers fetch n with `answers(n)`, `takes` after it starts, recording when it started and the
// `sent_at` it was given.
#[derive(Clone)]
pub struct Script {
    pub answers: Arc<dyn Fn(usize) -> Answer + Send + Sync>,
    pub fetched_at: Arc<Mutex<Vec<u64>>>,
    pub sent_at: Arc<Mutex<Vec<SystemTime>>>,
    pub started: Instant,
    pub refresh_interval: Duration,
    pub takes: Duration,
}

impl Script {
    pub fn new(clock: &Clock, answers: impl Fn(usize) -> Answer + Send + Sync + 'static) -> Self {
        Self {
            answers: Arc::new(answers),
            fetched_at: Arc::default(),
            sent_at: Arc::default(),
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
    async fn fetch(&self, sent_at: SystemTime) -> Answer {
        self.sent_at.lock().unwrap().push(sent_at);
        let fetches = {
            let mut fetched_at = self.fetched_at.lock().unwrap();
            fetched_at.push(self.started.elapsed().as_secs());
            fetched_at.len()
        };
        tokio::time::sleep(self.takes).await;
        (self.answers)(fetches - 1)
    }

    fn refresh_interval(&self) -> Duration {
        self.refresh_interval
    }
}

pub fn ticket(token: &str, expires_at: SystemTime) -> Answer {
    Ok(TicketAnswer::Ticket(OpaqueTicket {
        token: Secret::new(token.to_owned()),
        expires_at,
    }))
}

pub fn failed() -> Answer {
    Err("Rail Center was silent for 10s".to_owned())
}

// With no jitter, so the times the fetches come are exact.
pub fn start(clock: &Clock, script: &Script, refresh_interval: Duration) -> TicketHolder {
    start_jittered(clock, script, refresh_interval, Jitter::new(|| 1.0))
}

pub fn start_jittered(
    clock: &Clock,
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
pub async fn until(clock: &Clock, seconds: u64) {
    tokio::time::sleep_until(clock.started + Duration::from_secs(seconds)).await;
    tokio::task::yield_now().await;
}
