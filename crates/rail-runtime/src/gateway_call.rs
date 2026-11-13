//! One gateway call end to end: the waits, a slot to judge it in, and an answer by its deadline.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::available_parallelism;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_policy::{EnforcementMode, Fallback};
use tokio::sync::Semaphore;
use tokio::task::spawn_blocking;
use tokio::time::{Instant, timeout_at};

use crate::bundle_holder::{BundleHolder, HeldBundle};
use crate::callout::{DEFAULT_MAX_BODY_MB, MB};
use crate::denial::DenialReporter;
use crate::gateway::{Answer, Call, Unit, UnitSettings, decode_x_rail, judge_call};
use crate::key_set::KeySetHolder;
use crate::ticket_holder::WallClock;

// The longest a call waits for a bundle, and for a key it names: each within the deadline too.
const MAX_WAIT: Duration = Duration::from_secs(2);

// The deadline a setting past any `Instant` stands for.
const MAX_DEADLINE: Duration = Duration::from_secs(86_400);

/// How the gateway handles each call.
#[derive(Clone, Copy, Debug)]
pub struct GatewaySettings {
    /// From the call's arrival to its answer, the waits included. Any caller can spend both waits,
    /// 4 s, so a deadline not well past that answers its calls unjudged.
    pub deadline: Duration,
    /// How many calls are judged at once, each on a thread of its own.
    pub max_concurrent_judgements: NonZeroUsize,
    /// What a call gets while no bundle is held: `pass` forwards it, `block` answers 503.
    pub no_bundle_fallback: Fallback,
    /// The largest body judged; a call past it is refused unjudged, 413.
    pub max_body_bytes: usize,
    /// How each call is judged.
    pub unit: UnitSettings,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(5),
            max_concurrent_judgements: available_parallelism().unwrap_or(NonZeroUsize::MIN),
            no_bundle_fallback: Fallback::Pass,
            max_body_bytes: DEFAULT_MAX_BODY_MB * MB,
            unit: UnitSettings::default(),
        }
    }
}

/// The gateway role for a host: each call judged within its deadline, at most
/// `max_concurrent_judgements` at once.
pub struct Gateway {
    bundles: Arc<BundleHolder>,
    key_sets: Arc<KeySetHolder>,
    reporter: Arc<DenialReporter>,
    now: WallClock,
    judgements: Arc<Semaphore>,
    settings: GatewaySettings,
}

impl Gateway {
    /// Judges calls against what `bundles` and `key_sets` hold, denials reported to `reporter`,
    /// tickets read at `now`.
    pub fn new(
        bundles: Arc<BundleHolder>,
        key_sets: Arc<KeySetHolder>,
        reporter: Arc<DenialReporter>,
        now: WallClock,
        settings: GatewaySettings,
    ) -> Self {
        Self {
            bundles,
            key_sets,
            reporter,
            now,
            judgements: Arc::new(Semaphore::new(settings.max_concurrent_judgements.get())),
            settings,
        }
    }

    /// The largest body it judges; a host gathering a body can refuse it sooner.
    pub fn max_body_bytes(&self) -> usize {
        self.settings.max_body_bytes
    }

    /// Whether it's ready, and its state in readiness's words: `held` once a bundle is, else
    /// `none`.
    pub fn readiness(&self) -> (bool, &'static str) {
        match self.bundles.snapshot() {
            Some(_) => (true, "held"),
            None => (false, "none"),
        }
    }

    /// What to do with `call`, by its deadline, and the call back. Only a `POST` is judged, and
    /// no call whose body is past `max_body_bytes` is forwarded.
    pub async fn handle<B>(&self, call: Call<B>) -> (Answer, Call<B>)
    where
        B: AsRef<[u8]> + Clone + Send + Sync + 'static,
    {
        let bytes = call.body.as_ref().len();
        if bytes > self.settings.max_body_bytes {
            tracing::warn!(
                bytes,
                limit = self.settings.max_body_bytes,
                "refused unjudged: the body is past RAIL_MAX_BODY_MB"
            );
            return (Answer::TooLarge, call);
        }
        if call.method != "POST" {
            return (Answer::Forward, call);
        }
        let now = Instant::now();
        let deadline = now
            .checked_add(self.settings.deadline)
            .unwrap_or(now + MAX_DEADLINE);
        let within =
            |limit: Duration| limit.min(deadline.saturating_duration_since(Instant::now()));
        let Some(bundle) = self.bundles.wait_for_bundle(within(MAX_WAIT)).await else {
            let answer = answer_unjudged(None, self.settings.no_bundle_fallback);
            tracing::error!(?answer, "no policy bundle held; the call went unjudged");
            return (answer, call);
        };
        let ticket = decode_x_rail(&call);
        // Only where a signed ticket can be read and accepted: any caller can name a key.
        let judging = bundle.posture.mode != EnforcementMode::None;
        let keys = match ticket.kid() {
            Some(kid) if judging && names_tenant(&bundle) => {
                self.key_sets.wait_for_kid(kid, within(MAX_WAIT)).await
            }
            _ => self.key_sets.keys(),
        };
        let unjudged = |why: &str| {
            let answer = answer_unjudged(Some(&bundle), self.settings.no_bundle_fallback);
            tracing::error!(?answer, "{why}; the call went unjudged");
            answer
        };
        let Ok(Ok(slot)) = timeout_at(deadline, Arc::clone(&self.judgements).acquire_owned()).await
        else {
            return (
                unjudged("its deadline passed waiting for a free slot"),
                call,
            );
        };
        let call = Arc::new(call);
        let stop = Arc::new(AtomicBool::new(false));
        let _stop_if_dropped = StopOnDrop(Arc::clone(&stop));
        let unit = Unit {
            call: Arc::clone(&call),
            ticket,
            bundle: Arc::clone(&bundle),
            keys,
            now_epoch_seconds: epoch_seconds((self.now)()),
            settings: self.settings.unit,
            stop: Arc::clone(&stop),
        };
        let reporter = Arc::clone(&self.reporter);
        let mut worker = spawn_blocking(move || {
            let _slot = slot;
            judge_call(unit, &|denial| reporter.report(denial))
        });
        let finished = match timeout_at(deadline, &mut worker).await {
            Ok(finished) => finished,
            // Unless the worker claimed the answer first, with only its report left to make.
            Err(_) if stop.swap(true, Ordering::AcqRel) => worker.await,
            Err(_) => {
                let answer = unjudged("its deadline passed while it was judged");
                // Copied, with the worker still reading it.
                return (answer, Arc::unwrap_or_clone(call));
            }
        };
        let answer = match finished {
            Ok(answer) => answer,
            Err(error) if error.is_cancelled() => unjudged("judging it was cancelled"),
            // Not the panic's message, which may quote the call.
            Err(_) => unjudged("judging it panicked"),
        };
        (answer, Arc::unwrap_or_clone(call))
    }
}

// Stops the judgement once the call is answered or dropped: nobody waits for it then.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

// A signed ticket can only be accepted from a bundle naming the tenant's issuer and audience.
fn names_tenant(bundle: &HeldBundle) -> bool {
    let named = |claim: &Option<String>| claim.as_deref().is_some_and(|claim| !claim.is_empty());
    named(&bundle.ticket_issuer) && named(&bundle.ticket_audience)
}

// With a bundle, its `judge_fallback`, acted on at `enforce` only; without, the setting's.
fn answer_unjudged(bundle: Option<&HeldBundle>, no_bundle_fallback: Fallback) -> Answer {
    let fallback = match bundle {
        Some(bundle) if bundle.posture.mode == EnforcementMode::Enforce => {
            bundle.posture.judge_fallback
        }
        Some(_) => Fallback::Pass,
        None => no_bundle_fallback,
    };
    match fallback {
        Fallback::Pass => Answer::Forward,
        Fallback::Block => Answer::CannotApply,
    }
}

fn epoch_seconds(now: SystemTime) -> i64 {
    match now.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    }
}
