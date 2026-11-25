use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rail_model::agent::AgentId;
use rail_model::capture::{CompiledPlan, compile_plan, parse_plan};
use rail_model::redaction::REDACTION_VERSION;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use tokio::runtime::Handle;

use crate::capture_plan::CapturePlanSource;
use crate::client::{FetchError, Response, Unread, read_ok};
use crate::holder::{Attempt, Holder, Jitter, Kind, Step};
use crate::peek::{peek, schema_version, text};

// The one `schema_version` major read: any minor of it is.
const SCHEMA_MAJOR: u32 = 1;

const HASH_PREFIX: &str = "sha256:";
const HASH_HEX_DIGITS: usize = 64;

// The plan applied; only a plan applied or a 204 replaces it.
type Held = Option<Arc<CompiledPlan>>;

/// Keeps the capture plan applied through Rail Center failures and plans refused, for ever, until
/// Rail Center sends another it applies or says there's none; dropping it stops the refreshes.
pub struct CapturePlanHolder {
    holder: Holder<Held>,
}

impl CapturePlanHolder {
    /// Starts fetching the source's agent's plan at once, on `runtime`, and again every
    /// `refresh_interval`; after a failure that may pass, or a refusal while none is held, it backs
    /// off from 5 s up to 320 s or the interval, if shorter. `jitter` scales every wait.
    pub fn start(
        runtime: &Handle,
        source: impl CapturePlanSource,
        refresh_interval: Duration,
        jitter: Jitter,
    ) -> Self {
        let kind = CapturePlans {
            source,
            reported: AtomicBool::new(false),
        };
        Self {
            holder: Holder::start(runtime, kind, None, refresh_interval, jitter),
        }
    }

    /// The plan to capture under, or `None` if none is, never waiting; a new `Arc` only when the
    /// plan changes, so a window tells a change by pointer.
    pub fn snapshot(&self) -> Option<Arc<CompiledPlan>> {
        self.holder.snapshot().as_ref().clone()
    }
}

struct CapturePlans<S> {
    source: S,
    // Whether an answer was logged yet: the first no plan is said, as a change is.
    reported: AtomicBool,
}

// What one answer is: `Refused` blames the plan Rail Center sent, `Failed` the fetch or its body.
enum Outcome {
    // The plan held: an answer is only unchanged against one.
    Unchanged(Arc<CompiledPlan>),
    Replaced(Arc<CompiledPlan>),
    // `cleared`: a plan was held.
    NoPlan {
        cleared: bool,
    },
    Refused {
        refused_hash: String,
        reason: String,
    },
    Failed(Unread),
}

impl<S: CapturePlanSource> Kind for CapturePlans<S> {
    type Held = Held;
    type Answer = Result<Response, FetchError>;
    type Outcome = Outcome;

    const WHAT: &'static str = "capture plan";
    // It doesn't lapse, so only with none held is one sought sooner: at start, or after a 204.
    fn refusal_backs_off(&self, held: &Held) -> bool {
        held.is_none()
    }

    fn fetch(&self) -> impl Future<Output = Self::Answer> + Send {
        self.source.fetch()
    }

    fn step(&self, held: &Held, answer: Self::Answer) -> Step<Held, Outcome> {
        let outcome = read_answer(held.as_ref(), self.source.agent(), answer);
        let next = match &outcome {
            Outcome::Replaced(plan) => Some(Some(Arc::clone(plan))),
            Outcome::NoPlan { cleared: true } => Some(None),
            _ => None,
        };
        let attempt = match &outcome {
            Outcome::Unchanged(_) | Outcome::Replaced(_) | Outcome::NoPlan { .. } => {
                Attempt::Succeeded
            }
            Outcome::Refused { .. } => Attempt::Refused,
            Outcome::Failed(unread) => Attempt::of(unread),
        };
        Step {
            next,
            attempt,
            outcome,
        }
    }

    fn report(&self, outcome: Outcome, held: &Held, after_failure: bool, retry: u64) {
        let current = held.as_ref();
        let first = !self.reported.swap(true, Ordering::Relaxed);
        match outcome {
            // Said once, so a recovery shows without a line every poll.
            Outcome::Unchanged(plan) if after_failure => tracing::info!(
                content_hash = %plan.plan().content_hash,
                "capture plan fetched again, unchanged"
            ),
            Outcome::Unchanged(plan) => tracing::debug!(
                content_hash = %plan.plan().content_hash,
                "capture plan unchanged"
            ),
            Outcome::Replaced(plan) => {
                let read = plan.plan();
                tracing::info!(
                    content_hash = %read.content_hash,
                    plan_version = read.plan_version,
                    window_seconds = read.window_seconds,
                    entries = read.entries.len(),
                    refused_entries = plan.refused().count(),
                    entries_skipped = plan.entries_skipped(),
                    "holding a capture plan"
                );
            }
            Outcome::NoPlan { cleared } if cleared || first || after_failure => tracing::info!(
                "Rail Center holds no capture plan for this agent; capturing nothing"
            ),
            Outcome::NoPlan { .. } => tracing::debug!("still no capture plan for this agent"),
            Outcome::Refused {
                refused_hash,
                reason,
            } => log_refused(current, &reason, &refused_hash, retry),
            Outcome::Failed(Unread { reason, .. }) => log_fetch_failed(current, &reason, retry),
        }
    }
}

// At `warn`, where the bundle's are at `error`: with no plan the proxy only captures nothing.
fn log_refused(current: Option<&Arc<CompiledPlan>>, reason: &str, refused_hash: &str, retry: u64) {
    match current {
        Some(current) => tracing::warn!(
            reason,
            refused_hash,
            content_hash = %current.plan().content_hash,
            retry,
            "refusing the fetched capture plan; keeping the one held"
        ),
        None => tracing::warn!(
            reason,
            refused_hash,
            retry,
            "refusing the fetched capture plan; none is held"
        ),
    }
}

fn log_fetch_failed(current: Option<&Arc<CompiledPlan>>, reason: &str, retry: u64) {
    match current {
        Some(current) => tracing::warn!(
            reason,
            content_hash = %current.plan().content_hash,
            retry,
            "capture plan fetch failed; keeping the one held"
        ),
        None => tracing::warn!(reason, retry, "capture plan fetch failed; none is held"),
    }
}

// The fields read before the plan is, each as found, since a plan of another shape may hold any of
// them as anything.
#[derive(Deserialize)]
struct Peek {
    content_hash: Option<Value>,
    schema_version: Option<Value>,
    agent: Option<Value>,
}

fn read_answer(
    held: Option<&Arc<CompiledPlan>>,
    agent: &AgentId,
    answer: Result<Response, FetchError>,
) -> Outcome {
    if let Ok(Response {
        status: StatusCode::NO_CONTENT,
        ..
    }) = answer
    {
        return Outcome::NoPlan {
            cleared: held.is_some(),
        };
    }
    let body = match read_ok(answer) {
        Ok(body) => body,
        Err(unread) => return Outcome::Failed(unread),
    };
    let failed = |reason| Outcome::Failed(Unread::refused(reason));
    let not_a_plan = |reason| {
        failed(format!(
            "the capture plan isn't an object with one of each field read before it is ({reason})"
        ))
    };
    let (json, peek) = match peek::<Peek>(&body) {
        // A JSON array fills a struct's fields in order: only an object is a plan's shape.
        Ok((json, _))
            if !json
                .trim_start_matches([' ', '\t', '\n', '\r'])
                .starts_with('{') =>
        {
            return not_a_plan("not an object".to_owned());
        }
        Ok((json, Ok(peek))) => (json, peek),
        Ok((_, Err(reason))) => return not_a_plan(reason),
        Err(reason) => return failed(reason),
    };
    // Every reading of a plan is told by its hash, so one without a readable hash is a failed fetch.
    let Some(content_hash) = text(peek.content_hash.as_ref()).filter(|hash| is_content_hash(hash))
    else {
        return failed(format!(
            "the capture plan's content_hash isn't {HASH_PREFIX} and {HASH_HEX_DIGITS} lower-case \
             hex digits"
        ));
    };
    // The hash covers every field, so the same one is the plan held, whatever it was read as.
    if let Some(held) = held
        && held.plan().content_hash == content_hash
    {
        return Outcome::Unchanged(Arc::clone(held));
    }
    let named = peek
        .agent
        .as_ref()
        .and_then(|agent| AgentId::deserialize(agent).ok());
    if named.is_some_and(|named| named != *agent) {
        return failed(format!(
            "the capture plan names another agent than host_id {} and sandbox_name {}",
            agent.host_id, agent.sandbox_name
        ));
    }
    let refused = |reason| Outcome::Refused {
        refused_hash: content_hash.to_owned(),
        reason,
    };
    // Before the shape, so another major is refused for its version, not for what it holds.
    if let Some((declared, version)) = schema_version(peek.schema_version.as_ref())
        && version.major != SCHEMA_MAJOR
    {
        return refused(format!(
            "schema_version {declared} is a plan shape this proxy doesn't read; it reads \
             {SCHEMA_MAJOR}.x"
        ));
    }
    let plan = match parse_plan(json) {
        Ok(plan) => plan,
        Err(error) => return refused(error.to_string()),
    };
    // Last, so a mismatch describes only a plan the proxy would otherwise apply.
    if plan.redaction_version != REDACTION_VERSION {
        return refused(format!(
            "redaction_version {}: its paths were redacted by another version of the redaction \
             rule than this proxy's {REDACTION_VERSION}",
            plan.redaction_version
        ));
    }
    Outcome::Replaced(Arc::new(compile_plan(plan)))
}

fn is_content_hash(text: &str) -> bool {
    text.strip_prefix(HASH_PREFIX).is_some_and(|hex| {
        hex.len() == HASH_HEX_DIGITS
            && hex
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}
