//! How long a call to Rail Center waits to be made again: after attempts in a row that back off,
//! 5 s doubling to 320 s, jittered; and a wait as logged.

use std::hash::{BuildHasher, RandomState};
use std::sync::Arc;
use std::time::Duration;

// The first step, before the jitter: a lower one would let a fleet hammer Rail Center.
const FLOOR: Duration = Duration::from_secs(5);

// Six doublings of the floor: 320 s.
const MAX_DOUBLINGS: u32 = 6;

const MIN_JITTER: f64 = 0.8;
const MAX_JITTER: f64 = 1.2;

/// The factor every wait is scaled by, from 0.8 to 1.2, so a fleet doesn't call Rail Center in
/// lockstep.
#[derive(Clone)]
pub struct Jitter(Arc<dyn Fn() -> f64 + Send + Sync>);

impl Jitter {
    /// A random factor each time, from `std`'s per-process random keys.
    pub fn random() -> Self {
        Self::new(|| {
            // The top 53 bits, a uniform fraction in [0, 1).
            let fraction = (RandomState::new().hash_one(()) >> 11) as f64 / (1_u64 << 53) as f64;
            MIN_JITTER + (MAX_JITTER - MIN_JITTER) * fraction
        })
    }

    /// The factor `factor` returns.
    pub fn new(factor: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        Self(Arc::new(factor))
    }
}

/// The attempts in a row that backed off, and the wait each brings.
pub(crate) struct Backoff {
    failures: u32,
    jitter: Jitter,
}

impl Backoff {
    pub(crate) fn new(jitter: Jitter) -> Self {
        Self {
            failures: 0,
            jitter,
        }
    }

    /// Counts one more attempt in a row that backed off.
    pub(crate) fn failed(&mut self) {
        self.failures = self.failures.saturating_add(1);
    }

    /// The wait after the attempts counted: 5 s doubling to 320 s, or `at_most` if shorter,
    /// jittered.
    pub(crate) fn wait(&self, at_most: Option<Duration>) -> Duration {
        let wait = FLOOR * 2_u32.pow(self.failures.saturating_sub(1).min(MAX_DOUBLINGS));
        self.jittered(at_most.map_or(wait, |at_most| wait.min(at_most)))
    }

    /// Ends a run of failures.
    pub(crate) fn reset(&mut self) {
        self.failures = 0;
    }

    /// `wait`, jittered.
    pub(crate) fn jittered(&self, wait: Duration) -> Duration {
        wait.mul_f64((self.jitter.0)())
    }
}

/// Whole seconds, rounded: a jittered 5.9 s reads as 6.
pub(crate) fn round_to_seconds(wait: Duration) -> u64 {
    wait.as_secs() + u64::from(wait.subsec_millis() >= 500)
}
