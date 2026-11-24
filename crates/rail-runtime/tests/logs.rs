//! The log, captured through the field formatter `logging::init` installs.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rail_runtime::logging::{JsonLines, QuotedFields};
use tracing::subscriber::DefaultGuard;

use crate::holder::TestClock;
use crate::scripted::{Answer, Script};

#[derive(Clone, Default)]
pub struct Logs(Arc<Mutex<Vec<u8>>>);

impl Logs {
    /// Captures this thread's log until the guard drops.
    pub fn capture(&self) -> DefaultGuard {
        let writer = self.clone();
        let subscriber = tracing_subscriber::fmt()
            .fmt_fields(QuotedFields)
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_default(subscriber)
    }

    /// Captures this thread's log as JSON lines, every level, the level as `severity` if `severity`.
    pub fn capture_json(&self, severity: bool) -> DefaultGuard {
        let writer = self.clone();
        let subscriber = tracing_subscriber::fmt()
            .event_format(JsonLines { severity })
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::set_default(subscriber)
    }

    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl std::io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The lines of `logs` holding `text`.
pub fn lines_with<'a>(logs: &'a str, text: &str) -> Vec<&'a str> {
    logs.lines().filter(|line| line.contains(text)).collect()
}

/// The log of a holder `start` starts on a script answering `answers` in turn, once the last is
/// fetched.
pub async fn log_of<H>(answers: Vec<Answer>, start: impl FnOnce(&Script) -> H) -> String {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = TestClock::new();
    let count = answers.len();
    let script = Script::in_turn(&clock, answers);

    let _holder = start(&script);
    while script.fetched_at().len() < count {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    // No attempt comes under 5 s after the last, so this only lets it be logged.
    tokio::time::sleep(Duration::from_secs(1)).await;

    logs.text()
}
