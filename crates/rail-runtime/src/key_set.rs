//! The keys signed tickets are verified against, held for at most 900 s after the last good fetch.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use rail_policy::{DroppedKey, parse_key_set};
use reqwest::StatusCode;
use tokio::runtime::Handle;

use crate::client::{FetchError, Limits, RailCenterClient, Response};
use crate::clock::{Clock, Stamp};
use crate::holder::{Holder, Jitter, Kind, MIN_REFRESH_INTERVAL, Schedule, Step, round_to_seconds};
use crate::rail_center::{AUTH_MODE, AUTH_TOKEN};

const ROUTE: &str = "/v1/jwks.json";

// A tenant publishes a few keys of under 200 bytes each: past this a body is a fault.
const LIMITS: Limits = Limits {
    deadline: Duration::from_secs(10),
    silence: Duration::from_secs(5),
    max_bytes: 64 * 1024,
};

// Rail Center publishes a scheduled key at least this long before it signs with it.
const REFRESH_INTERVAL: Duration = Duration::from_secs(300);

// How long a withdrawn key verifies at a gateway cut off from Rail Center: three refreshes.
const MAX_AGE: Duration = Duration::from_secs(900);

// For the whole holder, not per `kid`, since each unknown one may be invented.
const UNKNOWN_KID_SPACING: Duration = Duration::from_secs(30);

/// Where key sets come from: Rail Center, or a test's script.
pub trait KeySource: Send + Sync + 'static {
    /// One fetch, whatever its status: what each means is the holder's.
    fn fetch(&self) -> impl Future<Output = Result<Response, FetchError>> + Send;
}

/// This tenant's key set, fetched from Rail Center.
pub struct RailCenterKeys {
    pub client: RailCenterClient,
}

impl KeySource for RailCenterKeys {
    async fn fetch(&self) -> Result<Response, FetchError> {
        self.client.get(ROUTE, &[], LIMITS).await
    }
}

/// A key set as fetched.
#[derive(Debug)]
pub struct KeySet {
    /// By `kid`.
    pub keys: HashMap<String, VerifyingKey>,
    fetched_at: Stamp,
}

/// Keeps Rail Center's key set, for 900 s at most after the last good fetch; dropping it stops
/// the refreshes.
pub struct KeySetHolder {
    holder: Holder<Option<Arc<KeySet>>>,
    clock: Clock,
}

impl KeySetHolder {
    /// Starts fetching at once, on `runtime`, and again every 300 s; after a failure it backs off
    /// from 5 s up to that. `clock` ages the set, and `jitter` scales every wait.
    pub fn start(runtime: &Handle, source: impl KeySource, clock: Clock, jitter: Jitter) -> Self {
        let schedule = Schedule {
            period: REFRESH_INTERVAL,
            initial_delay: MIN_REFRESH_INTERVAL,
            max_delay: REFRESH_INTERVAL,
        };
        let kind = Keys {
            source,
            clock: clock.clone(),
        };
        Self {
            holder: Holder::start(runtime, kind, None, schedule, jitter),
            clock,
        }
    }

    /// The set to verify with, or `None` if none was fetched or it's past its age, never
    /// waiting. The age is judged here, as it's read.
    pub fn keys(&self) -> Option<Arc<KeySet>> {
        let held = self.holder.snapshot();
        held.as_ref()
            .as_ref()
            .filter(|set| is_fresh(set, &self.clock))
            .cloned()
    }

    /// The set, once it holds `kid`. A `kid` not held asks for a refetch now, unless one started
    /// in the last 30 s, and waits up to `limit` for it: a replacement key signs as soon as it's
    /// published. It returns the set as it then is, which may still not hold `kid`.
    pub async fn wait_for_kid(&self, kid: &str, limit: Duration) -> Option<Arc<KeySet>> {
        let holds_kid = |held: &Option<Arc<KeySet>>| {
            held.as_ref()
                .is_some_and(|set| is_fresh(set, &self.clock) && set.keys.contains_key(kid))
        };
        if !holds_kid(&self.holder.snapshot()) {
            self.holder.refetch_now(UNKNOWN_KID_SPACING);
            self.holder.wait_until(holds_kid, limit).await;
        }
        self.keys()
    }
}

fn is_fresh(set: &KeySet, clock: &Clock) -> bool {
    clock.now().duration_since(set.fetched_at) <= MAX_AGE
}

struct Keys<S> {
    source: S,
    clock: Clock,
}

// What one answer did, and the entries meant as keys it dropped.
enum Outcome {
    Replaced {
        set: Arc<KeySet>,
        changed: bool,
        dropped: Vec<DroppedKey>,
    },
    Failed {
        reason: String,
        dropped: Vec<DroppedKey>,
    },
}

impl<S: KeySource> Kind for Keys<S> {
    type Held = Option<Arc<KeySet>>;
    type Answer = Result<Response, FetchError>;
    type Outcome = Outcome;

    const WHAT: &'static str = "signing key set";

    fn fetch(&self) -> impl Future<Output = Self::Answer> + Send {
        self.source.fetch()
    }

    fn step(&self, held: &Self::Held, answer: Self::Answer) -> Step<Self::Held, Outcome> {
        let (read, dropped) = read_answer(answer);
        let outcome = match read {
            Ok(keys) => {
                // A set aged out holds nothing, so the next one is news.
                let previous = held.as_ref().filter(|set| is_fresh(set, &self.clock));
                let changed = previous.is_none_or(|set| !same_kids(&set.keys, &keys));
                let set = Arc::new(KeySet {
                    keys,
                    fetched_at: self.clock.now(),
                });
                Outcome::Replaced {
                    set,
                    changed,
                    dropped,
                }
            }
            Err(reason) => Outcome::Failed { reason, dropped },
        };
        let next = match &outcome {
            Outcome::Replaced { set, .. } => Some(Some(Arc::clone(set))),
            Outcome::Failed { .. } => None,
        };
        Step {
            succeeded: next.is_some(),
            next,
            outcome,
        }
    }

    fn report(&self, outcome: Outcome, held: &Self::Held, _: bool, retry: Duration) {
        let retry = round_to_seconds(retry);
        let (Outcome::Replaced { dropped, .. } | Outcome::Failed { dropped, .. }) = &outcome;
        // At every fetch: Rail Center publishes none, so one is a fault, or not its answer.
        for DroppedKey { kid, reason } in dropped {
            tracing::warn!(
                kid = kid.as_deref(),
                reason,
                "dropping a signing key the fetched set lists"
            );
        }
        match outcome {
            // On a change only: most refreshes say what the last one said.
            Outcome::Replaced {
                set, changed: true, ..
            } => {
                tracing::info!(kids = %format_kids(&set.keys), "holding signing keys");
            }
            Outcome::Replaced { set, .. } => {
                tracing::debug!(kids = %format_kids(&set.keys), "signing keys unchanged");
            }
            Outcome::Failed { reason, .. } => {
                match held.as_ref().filter(|set| is_fresh(set, &self.clock)) {
                    Some(set) => tracing::warn!(
                        reason = %reason,
                        kids = %format_kids(&set.keys),
                        retry,
                        "signing key set fetch failed; keeping the set held"
                    ),
                    // Only a later fetch ends this state.
                    None => tracing::error!(
                        reason = %reason,
                        retry,
                        "signing key set fetch failed; none is held, so every signed ticket reads \
                         bad_signature"
                    ),
                }
            }
        }
    }
}

// The keys an answer brings, or why none, and the entries meant as keys it dropped.
fn read_answer(
    answer: Result<Response, FetchError>,
) -> (
    Result<HashMap<String, VerifyingKey>, String>,
    Vec<DroppedKey>,
) {
    let response = match answer {
        Ok(response) => response,
        Err(error) => return (Err(error.to_string()), Vec::new()),
    };
    if matches!(
        response.status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        let reason = format!(
            "Rail Center refused the credential ({}); every fetch fails this way until \
             {AUTH_MODE} and {AUTH_TOKEN} are fixed",
            response.status
        );
        return (Err(reason), Vec::new());
    }
    if response.status != StatusCode::OK {
        let reason = format!("Rail Center answered {}", response.status);
        return (Err(reason), Vec::new());
    }
    let Some(set) = parse_key_set(&response.body) else {
        let reason = "the set isn't an object of keys, or two entries share a kid";
        return (Err(reason.to_owned()), Vec::new());
    };
    if set.keys.is_empty() {
        return (Err("the set holds no usable key".to_owned()), set.dropped);
    }
    (Ok(set.keys), set.dropped)
}

fn same_kids(a: &HashMap<String, VerifyingKey>, b: &HashMap<String, VerifyingKey>) -> bool {
    a.len() == b.len() && a.keys().all(|kid| b.contains_key(kid))
}

// Sorted, so the same set reads the same in every line.
fn format_kids(keys: &HashMap<String, VerifyingKey>) -> String {
    let mut kids: Vec<&str> = keys.keys().map(String::as_str).collect();
    kids.sort_unstable();
    kids.join(", ")
}
