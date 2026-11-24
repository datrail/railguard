//! How a call to Rail Center is asked again after it brought nothing usable: 5 s doubling to
//! 320 s, every wait jittered.

use std::hash::{BuildHasher, RandomState};
use std::sync::Arc;
use std::time::Duration;

// The first wait: a lower one would let a fleet hammer Rail Center.
const FLOOR: Duration = Duration::from_secs(5);

// Six doublings of the floor.
const MAX_BACKOFF: Duration = Duration::from_secs(320);

// 5 s doubled 15 times is far past the 320 s cap, and the power can't overflow.
const MAX_DOUBLINGS: u32 = 15;

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

/// The failures in a row, and the wait each brings.
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

    /// The wait after one more failure in a row: 5 s doubling to 320 s, or `at_most` if shorter,
    /// jittered.
    pub(crate) fn failed(&mut self, at_most: Option<Duration>) -> Duration {
        self.failures = self.failures.saturating_add(1);
        let wait = (FLOOR * 2_u32.pow((self.failures - 1).min(MAX_DOUBLINGS))).min(MAX_BACKOFF);
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
