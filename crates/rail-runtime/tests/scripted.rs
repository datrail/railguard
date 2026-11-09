//! A scripted Rail Center source, for the bundle and key set holders' tests.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rail_runtime::{BundleSource, FetchError, KeySource, Response};
use reqwest::StatusCode;
use tokio::time::Instant;

use crate::holder::TestClock;

pub type Answer = Result<Response, FetchError>;

// A way of answering, called afresh for each fetch.
pub type Answering = fn() -> Answer;

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
        clock: &TestClock,
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

    async fn answer(&self) -> Answer {
        let fetches = {
            let mut fetched_at = self.fetched_at.lock().unwrap();
            fetched_at.push(self.started.elapsed().as_secs());
            fetched_at.len()
        };
        tokio::time::sleep(self.takes).await;
        (self.answers)(fetches - 1)
    }
}

impl BundleSource for Script {
    async fn fetch(&self) -> Answer {
        self.answer().await
    }
}

impl KeySource for Script {
    async fn fetch(&self) -> Answer {
        self.answer().await
    }
}

pub fn answer(status: StatusCode, body: &[u8]) -> Answer {
    Ok(Response {
        status,
        body: body.to_vec(),
    })
}
