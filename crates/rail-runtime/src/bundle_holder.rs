use std::sync::Arc;
use std::time::Duration;

use rail_policy::{
    CompiledBundle, EnforcementMode, Fallback, SchemaVersion, compile_bundle, parse_bundle,
};
use serde::Deserialize;
use serde_json::Value;
use serde_json::error::Category;
use tokio::runtime::Handle;

use crate::bundle::BundleSource;
use crate::client::{FetchError, Response, read_ok};
use crate::holder::{Holder, Jitter, Kind, MIN_REFRESH_INTERVAL, Schedule, Step, round_to_seconds};

// The one `schema_version` major read: any minor of it is.
const SCHEMA_MAJOR: u32 = 2;

/// A bundle that passed every check, compiled for judging.
#[derive(Debug)]
pub struct HeldBundle {
    pub compiled: CompiledBundle,
    /// Tells a changed bundle from the same one; it orders nothing.
    pub content_hash: String,
    pub schema_version: SchemaVersion,
    pub posture: Posture,
}

/// How much of a verdict is acted on, and what the fallbacks decide, as the bundle says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Posture {
    pub mode: EnforcementMode,
    pub binding_fallback: Fallback,
    pub ticket_fallback: Fallback,
}

impl Posture {
    // As the log tells it: the fallbacks only where they're acted on, at `enforce`.
    fn as_logged(self) -> (EnforcementMode, Option<(Fallback, Fallback)>) {
        let fallbacks = (self.mode == EnforcementMode::Enforce)
            .then_some((self.binding_fallback, self.ticket_fallback));
        (self.mode, fallbacks)
    }
}

// The last usable bundle; a failed fetch or a refused bundle never clears it.
type Held = Option<Arc<HeldBundle>>;

/// Keeps the last usable policy bundle through Rail Center failures, for ever; dropping it stops
/// the refreshes.
///
/// No bundle held is not an empty one, which would allow every call: it's that Rail Center hasn't
/// said what to enforce yet.
pub struct BundleHolder {
    holder: Holder<Held>,
}

impl BundleHolder {
    /// Starts fetching at once, on `runtime`, and again every `refresh_interval`; after a failure
    /// it backs off from 5 s up to the interval. `jitter` scales every wait.
    pub fn start(
        runtime: &Handle,
        source: impl BundleSource,
        refresh_interval: Duration,
        jitter: Jitter,
    ) -> Self {
        let schedule = Schedule {
            period: refresh_interval,
            initial_delay: MIN_REFRESH_INTERVAL,
            max_delay: refresh_interval,
        };
        let kind = Bundles { source };
        Self {
            holder: Holder::start(runtime, kind, None, schedule, jitter),
        }
    }

    /// The bundle held, or `None` if none was ever usable, never waiting.
    pub fn snapshot(&self) -> Option<Arc<HeldBundle>> {
        self.holder.snapshot().as_ref().clone()
    }

    /// The bundle held, waiting up to `limit` while none is, if an attempt is in flight or due
    /// within it. It never starts a fetch.
    pub async fn wait_for_bundle(&self, limit: Duration) -> Option<Arc<HeldBundle>> {
        self.holder
            .wait_until(Option::is_some, limit)
            .await
            .as_ref()
            .clone()
    }
}

struct Bundles<S> {
    source: S,
}

// What one answer is: `Refused` blames the bundle Rail Center sent, `Failed` the fetch.
enum Outcome {
    // The bundle held: an answer is only unchanged against one.
    Unchanged(Arc<HeldBundle>),
    Replaced {
        bundle: Arc<HeldBundle>,
        enabled_policies: usize,
        bindings: usize,
        // On a change only: most polls say what the last one said.
        posture_moved: bool,
    },
    Refused(String),
    Failed(String),
}

impl<S: BundleSource> Kind for Bundles<S> {
    type Held = Held;
    type Answer = Result<Response, FetchError>;
    type Outcome = Outcome;

    const WHAT: &'static str = "policy bundle";

    fn fetch(&self) -> impl Future<Output = Self::Answer> + Send {
        self.source.fetch()
    }

    fn step(&self, held: &Held, answer: Self::Answer) -> Step<Held, Outcome> {
        let outcome = read_answer(held.as_ref(), answer);
        let next = match &outcome {
            Outcome::Replaced { bundle, .. } => Some(Some(Arc::clone(bundle))),
            _ => None,
        };
        let succeeded = matches!(outcome, Outcome::Unchanged(_) | Outcome::Replaced { .. });
        Step {
            next,
            succeeded,
            outcome,
        }
    }

    fn report(&self, outcome: Outcome, held: &Held, after_failure: bool, retry: Duration) {
        let current = held.as_ref();
        let retry = round_to_seconds(retry);
        match outcome {
            // Said once, so a recovery shows without a line every poll.
            Outcome::Unchanged(bundle) if after_failure => tracing::info!(
                content_hash = %bundle.content_hash,
                "policy bundle fetched again, unchanged"
            ),
            Outcome::Unchanged(bundle) => {
                tracing::debug!(content_hash = %bundle.content_hash, "policy bundle unchanged");
            }
            Outcome::Replaced {
                bundle,
                enabled_policies,
                bindings,
                posture_moved,
            } => {
                let version = &bundle.schema_version;
                tracing::info!(
                    content_hash = %bundle.content_hash,
                    schema_version = %format_args!("{}.{}", version.major, version.minor),
                    enabled_policies,
                    bindings,
                    "holding a policy bundle"
                );
                if posture_moved {
                    log_posture(bundle.posture);
                }
            }
            Outcome::Refused(reason) => log_refused(current, &reason, retry),
            Outcome::Failed(reason) => log_fetch_failed(current, &reason, retry),
        }
    }
}

fn log_refused(current: Option<&Arc<HeldBundle>>, reason: &str, retry: u64) {
    match current {
        Some(current) => tracing::error!(
            reason = %reason,
            content_hash = %current.content_hash,
            retry,
            "refusing the fetched policy bundle; keeping the one held"
        ),
        None => tracing::error!(
            reason = %reason,
            retry,
            "refusing the fetched policy bundle; none is held"
        ),
    }
}

// An error once nothing is held: only a later fetch ends that state.
fn log_fetch_failed(current: Option<&Arc<HeldBundle>>, reason: &str, retry: u64) {
    match current {
        Some(current) => tracing::warn!(
            reason = %reason,
            content_hash = %current.content_hash,
            retry,
            "policy bundle fetch failed; keeping the one held"
        ),
        None => {
            tracing::error!(
                reason = %reason,
                retry,
                "policy bundle fetch failed; none is held"
            );
        }
    }
}

// What traffic now meets; what isn't held isn't said, since the holder doesn't know the caller's.
fn log_posture(posture: Posture) {
    match posture.mode {
        EnforcementMode::None => tracing::info!(
            enforcement = "none",
            "posture: Rail Center says judge nothing; every request is forwarded unjudged, and \
             polling goes on"
        ),
        EnforcementMode::Observe => tracing::info!(
            enforcement = "observe",
            "posture: every request is judged and its verdict logged; nothing is refused"
        ),
        EnforcementMode::Enforce => tracing::info!(
            enforcement = "enforce",
            binding_fallback = format_fallback(posture.binding_fallback),
            ticket_fallback = format_fallback(posture.ticket_fallback),
            "posture: every request is judged, and one denied is refused"
        ),
    }
}

fn format_fallback(fallback: Fallback) -> &'static str {
    match fallback {
        Fallback::Pass => "pass",
        Fallback::Block => "block",
    }
}

// The two fields read before the bundle is.
#[derive(Default, Deserialize)]
struct Peek {
    content_hash: Option<Value>,
    schema_version: Option<Value>,
}

fn read_answer(held: Option<&Arc<HeldBundle>>, answer: Result<Response, FetchError>) -> Outcome {
    let body = match read_ok(answer) {
        Ok(body) => body,
        Err(reason) => return Outcome::Failed(reason),
    };
    let Ok(json) = std::str::from_utf8(&body) else {
        return Outcome::Failed("the response isn't UTF-8".to_owned());
    };
    let peek = match serde_json::from_str::<Peek>(json) {
        Ok(peek) => peek,
        // JSON, but not an object: `parse_bundle` says what it is.
        Err(error) if error.classify() == Category::Data => Peek::default(),
        Err(error) => {
            return Outcome::Failed(format!(
                "the response isn't JSON ({:?} error at line {}, column {})",
                error.classify(),
                error.line(),
                error.column()
            ));
        }
    };
    // The hash covers every field that changes behaviour, so the same one is the bundle held.
    let hash = peek.content_hash.as_ref().and_then(Value::as_str);
    if let Some(held) = held
        && hash == Some(held.content_hash.as_str())
    {
        return Outcome::Unchanged(Arc::clone(held));
    }
    // Before the shape, so another major is refused for its version, not for what it holds.
    if let Some(Value::String(declared)) = &peek.schema_version
        && let Ok(version) = SchemaVersion::try_from(declared.clone())
        && version.major != SCHEMA_MAJOR
    {
        return Outcome::Refused(format!(
            "schema_version {declared} is a bundle shape this gateway doesn't read; it reads \
             {SCHEMA_MAJOR}.x"
        ));
    }
    let bundle = match parse_bundle(json) {
        Ok(bundle) => bundle,
        Err(error) => return Outcome::Refused(error.to_string()),
    };
    let enabled_policies = bundle
        .policies
        .iter()
        .filter(|policy| policy.enabled)
        .count();
    let bindings = bundle.bindings.len();
    let posture = Posture {
        mode: bundle.enforcement.mode,
        binding_fallback: bundle.binding_fallback,
        ticket_fallback: bundle.ticket_fallback,
    };
    Outcome::Replaced {
        bundle: Arc::new(HeldBundle {
            compiled: compile_bundle(&bundle),
            content_hash: bundle.content_hash,
            schema_version: bundle.schema_version,
            posture,
        }),
        enabled_policies,
        bindings,
        posture_moved: held.is_none_or(|held| held.posture.as_logged() != posture.as_logged()),
    }
}
