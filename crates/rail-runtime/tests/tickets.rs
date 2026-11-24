//! A scripted ticket source's answers and the holder started on it, for the ticket holder's tests.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rail_runtime::{Jitter, TicketHolder, Unavailable};
use reqwest::StatusCode;
use time::OffsetDateTime;
use tokio::runtime::Handle;

use crate::holder::{HOUR, TestClock, until};
pub use crate::scripted::{Answer, Script, agent, answer, failed};

// A ticket entry, as Rail Center writes one.
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

// The script's agent's one ticket.
pub fn one(token: &str, expires_at: &str) -> String {
    let agent = agent();
    let sandbox = format!(r#""{}""#, agent.sandbox_name);
    envelope(&agent.host_id, &[entry(&sandbox, token, expires_at)])
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
        HOUR,
        Arc::new(move || now),
        Jitter::new(|| 1.0),
    );
    until(&clock, 0).await;
    holder
}

// The script's agent's ticket, expiring at `expires_at`.
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
    answer(StatusCode::OK, one(token, &expires_at))
}

pub fn no_ticket() -> Answer {
    answer(StatusCode::OK, envelope(&agent().host_id, &[]))
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
    let source = script.clone();
    TicketHolder::start(
        &Handle::current(),
        source,
        refresh_interval,
        clock.wall(),
        jitter,
    )
}

pub fn token(holder: &TicketHolder) -> Result<String, Unavailable> {
    holder
        .current()
        .map(|ticket| ticket.token.expose().to_owned())
}
