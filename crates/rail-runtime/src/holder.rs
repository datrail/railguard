//! One shape for what Rail Center hands out: a value read lock-free, replaced by one refresh loop.

use std::future::{Future, poll_fn};
use std::hash::{BuildHasher, RandomState};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use arc_swap::ArcSwap;
use tokio::runtime::Handle;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use crate::ticket::MIN_REFRESH_INTERVAL;

const MIN_JITTER: f64 = 0.8;
const MAX_JITTER: f64 = 1.2;

/// One kind of value: how it's fetched, and what an answer makes of what's held.
pub(crate) trait Kind: Send + Sync + 'static {
    type Held: Send + Sync + 'static;
    type Answer: Send;
    type Outcome: Send;

    /// Names the kind in the loop's own log lines.
    const WHAT: &'static str;

    fn fetch(&self) -> impl Future<Output = Self::Answer> + Send;

    /// What `answer` does to `held`. A panic here, or in `fetch`, is a failed attempt.
    fn step(&self, held: &Self::Held, answer: Self::Answer) -> Step<Self::Held, Self::Outcome>;

    /// Logs what a step did, once the wait before the next attempt is known.
    fn report(&self, outcome: Self::Outcome, held: &Self::Held, next_attempt_in: Duration);
}

/// What one attempt decided.
pub(crate) struct Step<T, O> {
    /// The value to hold from now on; `None` keeps the one held.
    pub(crate) next: Option<T>,
    /// A failure backs off; a success resumes the period.
    pub(crate) succeeded: bool,
    /// Shortens the wait before the next attempt, never lengthens it.
    pub(crate) at_most: Option<Duration>,
    pub(crate) outcome: O,
}

/// When a kind's attempts come, before the jitter and the 5 s floor.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Schedule {
    /// From the start of an attempt that succeeded to the next one.
    pub(crate) period: Duration,
    /// After the first failure, doubling after each one that follows.
    pub(crate) initial_delay: Duration,
    pub(crate) max_delay: Duration,
}

/// The factor every wait is scaled by, from 0.8 to 1.2, so a fleet doesn't poll in lockstep.
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

    /// The factor `factor` returns, held to 0.8–1.2.
    pub fn new(factor: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        Self(Arc::new(factor))
    }

    // A NaN reads as the lowest, since `clamp` would pass it through.
    fn factor(&self) -> f64 {
        let factor = (self.0)();
        if factor.is_nan() {
            MIN_JITTER
        } else {
            factor.clamp(MIN_JITTER, MAX_JITTER)
        }
    }
}

/// A value fetched and refreshed on `runtime`; dropping it stops the refreshes.
pub(crate) struct Holder<T> {
    held: Arc<ArcSwap<T>>,
    refresh: AbortHandle,
}

impl<T: Send + Sync + 'static> Holder<T> {
    /// Holds `initial` and fetches at once.
    pub(crate) fn start<K: Kind<Held = T>>(
        runtime: &Handle,
        kind: K,
        initial: T,
        schedule: Schedule,
        jitter: Jitter,
    ) -> Self {
        let held = Arc::new(ArcSwap::from_pointee(initial));
        let refresh = runtime.spawn(run(kind, Arc::clone(&held), schedule, jitter));
        let abort = refresh.abort_handle();
        // Attempts catch their panics, so this is a defect in the loop, which would end quietly.
        runtime.spawn(async move {
            if refresh.await.is_err_and(|error| error.is_panic()) {
                tracing::error!(
                    "the {} refresh loop panicked; the {} is no longer refreshed",
                    K::WHAT,
                    K::WHAT
                );
            }
        });
        Self {
            held,
            refresh: abort,
        }
    }

    /// What's held now, never waiting.
    pub(crate) fn snapshot(&self) -> Arc<T> {
        self.held.load_full()
    }
}

impl<T> Drop for Holder<T> {
    fn drop(&mut self) {
        self.refresh.abort();
    }
}

// The one writer.
async fn run<K: Kind>(kind: K, held: Arc<ArcSwap<K::Held>>, schedule: Schedule, jitter: Jitter) {
    // Failed attempts since the last success.
    let mut failures: u32 = 0;
    loop {
        let started = Instant::now();
        let step = attempt(&kind, &held).await;
        let took = started.elapsed();
        let (at_most, outcome) = match step {
            Some(step) => {
                if let Some(next) = step.next {
                    held.store(Arc::new(next));
                }
                failures = if step.succeeded {
                    0
                } else {
                    failures.saturating_add(1)
                };
                (step.at_most, Some(step.outcome))
            }
            None => {
                failures = failures.saturating_add(1);
                // Not the panic's message, which may quote what Rail Center sent.
                tracing::error!(
                    "a {} refresh attempt panicked; keeping what's held",
                    K::WHAT
                );
                (None, None)
            }
        };
        let wait = next_wait(&schedule, failures, at_most, jitter.factor());
        // A period runs from the start of the attempt, so a slow fetch doesn't shift the cadence.
        let wait = if failures == 0 {
            wait.saturating_sub(took).max(MIN_REFRESH_INTERVAL)
        } else {
            wait
        };
        if let Some(outcome) = outcome {
            kind.report(outcome, &held.load(), wait);
        }
        tokio::time::sleep(wait).await;
    }
}

// One fetch and its step, `None` if either panicked.
async fn attempt<K: Kind>(kind: &K, held: &ArcSwap<K::Held>) -> Option<Step<K::Held, K::Outcome>> {
    let mut fetch = pin!(kind.fetch());
    // A panicking future is never polled again: it's dropped as soon as this returns.
    let answer =
        poll_fn(
            |context| match catch_unwind(AssertUnwindSafe(|| fetch.as_mut().poll(context))) {
                Ok(poll) => poll.map(Some),
                Err(_) => Poll::Ready(None),
            },
        )
        .await?;
    catch_unwind(AssertUnwindSafe(|| kind.step(&held.load(), answer))).ok()
}

// The wait before the next attempt: the period after a success, else the backoff; jittered, and
// never under the floor.
fn next_wait(
    schedule: &Schedule,
    failures: u32,
    at_most: Option<Duration>,
    factor: f64,
) -> Duration {
    let wait = match failures {
        0 => schedule.period,
        _ => schedule
            .initial_delay
            .saturating_mul(2_u32.saturating_pow(failures - 1))
            .min(schedule.max_delay),
    };
    let wait = at_most.map_or(wait, |at_most| wait.min(at_most));
    Duration::try_from_secs_f64(wait.as_secs_f64() * factor)
        .unwrap_or(Duration::MAX)
        .max(MIN_REFRESH_INTERVAL)
}
