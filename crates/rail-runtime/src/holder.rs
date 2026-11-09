//! One shape for what Rail Center hands out: a value read lock-free, replaced by one refresh loop.

use std::future::{Future, poll_fn};
use std::hash::{BuildHasher, RandomState};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::sync::{Arc, Weak};
use std::task::Poll;
use std::time::Duration;

use arc_swap::ArcSwap;
use rail_policy::sanitize;
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio::time::{Instant, timeout_at};

// Not settings: a lower floor would let a fleet hammer Rail Center.
pub(crate) const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

// A day: a ticket lives no longer, and a bundle polled less often is surely a mistake.
const MAX_REFRESH_INTERVAL: Duration = Duration::from_secs(86_400);

// 5 s doubled 15 times is past a day, the longest maximum delay, and the power can't overflow.
const MAX_DOUBLINGS: u32 = 15;

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

    /// What `answer` does to `held`. A panic here, or polling `fetch`, is a failed attempt.
    fn step(&self, held: &Self::Held, answer: Self::Answer) -> Step<Self::Held, Self::Outcome>;

    /// A cap on the wait before the next attempt, from what's held, after a panic too.
    fn wait_at_most(&self, _held: &Self::Held) -> Option<Duration> {
        None
    }

    /// Logs what a step did, once the wait before the next attempt is known; `after_failure` is
    /// whether the attempt before it failed, a panic included.
    fn report(
        &self,
        outcome: Self::Outcome,
        held: &Self::Held,
        after_failure: bool,
        next_attempt_in: Duration,
    );
}

/// What one attempt decided.
pub(crate) struct Step<T, O> {
    /// The value to hold from now on; `None` keeps the one held.
    pub(crate) next: Option<T>,
    /// A failure backs off; a success resumes the period.
    pub(crate) succeeded: bool,
    pub(crate) outcome: O,
}

/// A refresh interval as set: whole seconds, from the 5 s floor to a day.
pub(crate) fn parse_refresh_interval(text: &str) -> Result<Duration, String> {
    match text.parse::<u64>().map(Duration::from_secs) {
        // The holder never refreshes faster, so a lower interval would be silently raised.
        Ok(interval) if (MIN_REFRESH_INTERVAL..=MAX_REFRESH_INTERVAL).contains(&interval) => {
            Ok(interval)
        }
        _ => Err(format!(
            "must be a whole number of seconds, from {} to {}, got: {}",
            MIN_REFRESH_INTERVAL.as_secs(),
            MAX_REFRESH_INTERVAL.as_secs(),
            sanitize(text)
        )),
    }
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

    /// The factor `factor` returns.
    pub fn new(factor: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        Self(Arc::new(factor))
    }

    fn factor(&self) -> f64 {
        (self.0)()
    }
}

// Where the loop is, for a reader deciding whether waiting can bring anything.
#[derive(Clone, Copy)]
enum Progress {
    InFlight,
    // `started`: when the attempt that just ended began.
    NextAt { next: Instant, started: Instant },
}

/// A value fetched and refreshed on `runtime`; dropping it stops the refreshes.
pub(crate) struct Holder<T> {
    held: Arc<ArcSwap<T>>,
    progress: watch::Receiver<Progress>,
    // Weak, so the sender goes with the loop and a waiter sees the loop is gone.
    publish: Weak<watch::Sender<Progress>>,
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
        let (publish, progress) = watch::channel(Progress::InFlight);
        let publish = Arc::new(publish);
        let weak = Arc::downgrade(&publish);
        let refresh = runtime
            .spawn(run(kind, Arc::clone(&held), publish, schedule, jitter))
            .abort_handle();
        Self {
            held,
            progress,
            publish: weak,
            refresh,
        }
    }

    /// What's held now, never waiting.
    pub(crate) fn snapshot(&self) -> Arc<T> {
        self.held.load_full()
    }

    /// What's held once `holds` it, or once `limit` has passed, or at once if no attempt is in
    /// flight or due before then. It never starts a fetch, and is awaited on the caller's runtime.
    pub(crate) async fn wait_until(&self, holds: impl Fn(&T) -> bool, limit: Duration) -> Arc<T> {
        let deadline = Instant::now() + limit;
        let mut progress = self.progress.clone();
        loop {
            // Before what's held: the loop stores a value before it says the attempt ended.
            let at = *progress.borrow_and_update();
            let held = self.snapshot();
            let attempt_due = match at {
                Progress::InFlight => true,
                Progress::NextAt { next, .. } => next < deadline,
            };
            if holds(&held) || !attempt_due {
                return held;
            }
            // The limit passed, or the loop is gone.
            if !timeout_at(deadline, progress.changed())
                .await
                .is_ok_and(|changed| changed.is_ok())
            {
                return self.snapshot();
            }
        }
    }

    /// Brings the next attempt forward to now, unless one is in flight or the last started within
    /// `spacing`, the backoff notwithstanding. A reader then waits for it with `wait_until`.
    pub(crate) fn refetch_now(&self, spacing: Duration) {
        let Some(publish) = self.publish.upgrade() else {
            return;
        };
        let now = Instant::now();
        // Read and changed under the watch's lock, so concurrent callers bring one attempt.
        publish.send_if_modified(|progress| match progress {
            Progress::NextAt { next, started } if *next > now && now - *started >= spacing => {
                *next = now;
                true
            }
            _ => false,
        });
    }
}

impl<T> Drop for Holder<T> {
    fn drop(&mut self) {
        self.refresh.abort();
    }
}

// The one writer.
async fn run<K: Kind>(
    kind: K,
    held: Arc<ArcSwap<K::Held>>,
    publish: Arc<watch::Sender<Progress>>,
    schedule: Schedule,
    jitter: Jitter,
) {
    let mut progress = publish.subscribe();
    // Failed attempts since the last success.
    let mut failures: u32 = 0;
    loop {
        publish.send_replace(Progress::InFlight);
        let started = Instant::now();
        let after_failure = failures > 0;
        let step = attempt(&kind, &held).await;
        let took = started.elapsed();
        let outcome = match step {
            Some(step) => {
                if let Some(next) = step.next {
                    held.store(Arc::new(next));
                }
                failures = if step.succeeded { 0 } else { failures + 1 };
                Some(step.outcome)
            }
            None => {
                failures += 1;
                // Not the panic's message, which may quote what Rail Center sent.
                tracing::error!(
                    "a {} refresh attempt panicked; keeping what's held",
                    K::WHAT
                );
                None
            }
        };
        let wait = {
            let held = held.load();
            let wait = next_wait(&schedule, failures, jitter.factor());
            // A period runs from the start of the attempt, so a slow fetch doesn't shift the cadence.
            let wait = if failures == 0 {
                // A fetch may outlast a short period: then the floor below applies.
                wait.saturating_sub(took)
            } else {
                wait
            };
            // After the jitter, so a kind's cap is never exceeded.
            let wait = kind
                .wait_at_most(&held)
                .map_or(wait, |at_most| wait.min(at_most))
                .max(MIN_REFRESH_INTERVAL);
            if let Some(outcome) = outcome {
                kind.report(outcome, &held, after_failure, wait);
            }
            wait
        };
        publish.send_replace(Progress::NextAt {
            next: Instant::now() + wait,
            started,
        });
        sleep_until_due(&mut progress).await;
    }
}

// Until the next attempt is due, which `refetch_now` may bring forward.
async fn sleep_until_due(progress: &mut watch::Receiver<Progress>) {
    loop {
        let Progress::NextAt { next, .. } = *progress.borrow_and_update() else {
            return;
        };
        if timeout_at(next, progress.changed()).await.is_err() {
            return;
        }
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

// The period after a success, else the backoff; jittered.
fn next_wait(schedule: &Schedule, failures: u32, factor: f64) -> Duration {
    let wait = match failures {
        0 => schedule.period,
        _ => (schedule.initial_delay * 2_u32.pow((failures - 1).min(MAX_DOUBLINGS)))
            .min(schedule.max_delay),
    };
    wait.mul_f64(factor)
}

// Whole seconds, rounded: a jittered 5.9 s reads as 6.
pub(crate) fn round_to_seconds(wait: Duration) -> u64 {
    wait.as_secs() + u64::from(wait.subsec_millis() >= 500)
}
