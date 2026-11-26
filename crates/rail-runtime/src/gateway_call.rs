//! One gateway call end to end: the waits, a slot to judge it in, and an answer by its deadline.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::available_parallelism;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_model::body::{BodyRead, ByteBudget, RequestBody, Unread};
use rail_model::policy::{EnforcementMode, Fallback};
use rail_model::request::ObservedRequest;
use tokio::sync::Semaphore;
use tokio::task::spawn_blocking;
use tokio::time::{Instant, timeout_at};

use crate::bundle_holder::{BundleHolder, HeldBundle};
use crate::callout::{GATEWAY_MAX_BODY_MB, MB};
use crate::clock::WallClock;
use crate::denial::DenialReporter;
use crate::gateway::{
    Answer, Unit, UnitSettings, answer_by, answer_unjudged, decode_x_rail, judge_call,
};
use crate::gather::{BODY_BUDGET_BYTES, BodyEnding, RequestGatherer};
use crate::key_set::KeySetHolder;

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
    /// Whether a body shorter than its `Content-Length` is cut short: not where a host may hand
    /// over a body decoded with its length as sent.
    pub checks_declared_length: bool,
    /// How each call is judged.
    pub unit: UnitSettings,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(5),
            max_concurrent_judgements: available_parallelism().unwrap_or(NonZeroUsize::MIN),
            no_bundle_fallback: Fallback::Pass,
            max_body_bytes: GATEWAY_MAX_BODY_MB * MB,
            checks_declared_length: true,
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
    budget: Arc<ByteBudget>,
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
            budget: ByteBudget::new(BODY_BUDGET_BYTES),
            settings,
        }
    }

    /// The call whose header lines are `lines`, its body to gather within the cap; a body adopted
    /// past the budget is kept, to send on.
    pub fn gather<N, V>(&self, lines: impl IntoIterator<Item = (N, V)>) -> RequestGatherer
    where
        N: AsRef<[u8]>,
        V: AsRef<[u8]> + Into<Vec<u8>>,
    {
        let mut gatherer =
            RequestGatherer::keeping_lines(lines, self.settings.max_body_bytes, &self.budget);
        gatherer.keep_past_budget();
        if !self.settings.checks_declared_length {
            gatherer.ignore_declared_length();
        }
        gatherer
    }

    /// Whether it's ready, and its state in readiness's words: `held` once a bundle is, else
    /// `none`.
    pub fn readiness(&self) -> (bool, &'static str) {
        match self.bundles.snapshot() {
            Some(_) => (true, "held"),
            None => (false, "none"),
        }
    }

    /// What to do with the call `gatherer` gathered, its body ended as `ending`, by its deadline,
    /// and its body back for the host to send on. Only a `POST` is judged, and no call whose body
    /// is past `max_body_bytes`, cut short, or dropped past the memory budget is forwarded.
    pub async fn handle(&self, gatherer: RequestGatherer, ending: BodyEnding) -> (Answer, Vec<u8>) {
        let call = Arc::new(gatherer.finish(ending));
        let answer = self.answer(&call).await;
        (answer, into_bytes(call))
    }

    async fn answer(&self, call: &Arc<ObservedRequest>) -> Answer {
        let bytes = call.body_bytes();
        match call.body() {
            BodyRead::Unread(Unread::PastCap) => {
                tracing::warn!(
                    bytes,
                    limit = self.settings.max_body_bytes,
                    "refused unjudged: the body is past RAIL_MAX_BODY_MB"
                );
                return Answer::TooLarge;
            }
            // Only chunks gathered past it: its bytes are gone, so it can't be sent on.
            BodyRead::Unread(Unread::PastBudget) => {
                tracing::error!(
                    bytes,
                    "refused unjudged: the body is past the memory budget"
                );
                return Answer::CannotApply;
            }
            // A body cut short could read as another call, so it's never judged as whole.
            BodyRead::Unread(Unread::CutShort) => {
                tracing::warn!(bytes, "refused unjudged: the host sent part of the body");
                return Answer::TooLarge;
            }
            _ if call.method() != "POST" => return Answer::Forward,
            // A host's own not sent is logged by it, which knows what it wasn't configured to send;
            // a declared length with nothing sent isn't.
            BodyRead::Unread(Unread::NotSent) => return Answer::CannotApply,
            BodyRead::Whole(_) => {}
        }
        let now = Instant::now();
        let deadline = now
            .checked_add(self.settings.deadline)
            .unwrap_or(now + MAX_DEADLINE);
        let within =
            |limit: Duration| limit.min(deadline.saturating_duration_since(Instant::now()));
        let Some(bundle) = self.bundles.wait_for_bundle(within(MAX_WAIT)).await else {
            let answer = answer_by(self.settings.no_bundle_fallback);
            tracing::error!(?answer, "no policy bundle held; the call went unjudged");
            return answer;
        };
        let ticket = decode_x_rail(call);
        // Only where a signed ticket can be read and accepted: any caller can name a key.
        let judging = bundle.posture.mode != EnforcementMode::None;
        let keys = match ticket.kid() {
            Some(kid) if judging && names_tenant(&bundle) => {
                self.key_sets.wait_for_kid(kid, within(MAX_WAIT)).await
            }
            _ => self.key_sets.keys(),
        };
        let unjudged = |why: &str| answer_unjudged(&bundle, why);
        let Ok(Ok(slot)) = timeout_at(deadline, Arc::clone(&self.judgements).acquire_owned()).await
        else {
            return unjudged("its deadline passed waiting for a free slot");
        };
        let stop = Arc::new(AtomicBool::new(false));
        let _stop_if_dropped = StopOnDrop(Arc::clone(&stop));
        let unit = Unit {
            call: Arc::clone(call),
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
            Err(_) => return unjudged("its deadline passed while it was judged"),
        };
        match finished {
            Ok(answer) => answer,
            Err(error) if error.is_cancelled() => unjudged("judging it was cancelled"),
            // Not the panic's message, which may quote the call.
            Err(_) => unjudged("judging it panicked"),
        }
    }
}

// Its body's bytes, moved out unless a judgement past its deadline still reads them: then copied.
fn into_bytes(call: Arc<ObservedRequest>) -> Vec<u8> {
    let BodyRead::Whole(body) = call.body().clone() else {
        return Vec::new();
    };
    drop(call);
    Arc::try_unwrap(body).map_or_else(|shared| shared.as_bytes().to_vec(), RequestBody::into_bytes)
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

fn epoch_seconds(now: SystemTime) -> i64 {
    match now.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    }
}
