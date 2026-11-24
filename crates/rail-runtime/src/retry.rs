//! The jitter every wait for Rail Center is scaled by.

use std::hash::{BuildHasher, RandomState};
use std::sync::Arc;
use std::time::Duration;

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

    pub(crate) fn factor(&self) -> f64 {
        (self.0)()
    }
}

/// Whole seconds, rounded: a jittered 5.9 s reads as 6.
pub(crate) fn round_to_seconds(wait: Duration) -> u64 {
    wait.as_secs() + u64::from(wait.subsec_millis() >= 500)
}
