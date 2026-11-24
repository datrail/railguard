//! A clock on tokio's paused time, for the holders' tests.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_runtime::WallClock;
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

// Lets the refresh loop run up to `seconds` from the start.
pub async fn until(clock: &TestClock, seconds: u64) {
    tokio::time::sleep_until(clock.started + Duration::from_secs(seconds)).await;
    tokio::task::yield_now().await;
}
