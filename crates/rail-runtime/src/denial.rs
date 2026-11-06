//! Denial reports, sent to Rail Center without the call waiting for them.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rail_policy::{Decision, Endpoint, FallbackDenial, Outcome, TicketState};
use reqwest::StatusCode;
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::client::{FetchError, Limits, RailCenterClient, Response, read_status};
use crate::ticket_holder::WallClock;

const ROUTE: &str = "/v1/denials";

// Nothing waits on a report, so a slow Rail Center costs a dropped report, not a task outliving
// its call by long. Rail Center answers a few bytes.
const LIMITS: Limits = Limits {
    deadline: Duration::from_secs(5),
    silence: Duration::from_secs(5),
    max_bytes: 64 * 1024,
};

// Past this, a report is dropped rather than queued: a caller can make one per request.
const MAX_IN_FLIGHT: usize = 64;

// A fallback denial needs nothing from the caller, so its reports can't take the whole budget
// from those naming a policy.
const MAX_IN_FLIGHT_NAMING_NO_POLICY: usize = 16;

// The caller's text, stored by Rail Center unbounded: a status is a few words, past this a payload.
const MAX_CLAIMED_STATUS_CHARS: usize = 255;

/// Where reports go: Rail Center, or a test's script.
pub trait DenialSink: Send + Sync + 'static {
    /// One attempt to deliver `body`, whatever the status: what each means is the reporter's.
    fn send(&self, body: Vec<u8>) -> impl Future<Output = Result<Response, FetchError>> + Send;
}

/// Reports sent to Rail Center.
pub struct RailCenterDenials {
    pub client: RailCenterClient,
}

impl DenialSink for RailCenterDenials {
    async fn send(&self, body: Vec<u8>) -> Result<Response, FetchError> {
        self.client.post(ROUTE, body, LIMITS).await
    }
}

/// What denied a call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeniedBy {
    /// The `block` policy whose condition held.
    Policy(Uuid),
    /// A fallback, naming no policy.
    Fallback(FallbackDenial),
}

impl DeniedBy {
    /// What denies the call in `outcome`: its first fallback denial, else the policy that denied
    /// it. `None` if nothing did, or if the walk was refused, which is never reported.
    pub fn of(outcome: &Outcome) -> Option<Self> {
        if let Some(&fallback) = outcome.fallback_denials.first() {
            return Some(Self::Fallback(fallback));
        }
        match outcome.chain.as_ref()?.decision {
            Decision::Denied(policy) => Some(Self::Policy(policy)),
            Decision::Allowed | Decision::Refused { .. } => None,
        }
    }
}

/// What a report is built from: no claim is copied into it.
#[derive(Clone, Copy, Debug)]
pub struct Denial<'a> {
    pub denied_by: DeniedBy,
    pub endpoint: &'a Endpoint,
    pub ticket: &'a TicketState,
    /// The `x-rail-status` the caller sent, if it sent one value: what it says, never believed.
    pub claimed_status: Option<&'a str>,
}

// The body of `POST /v1/denials`, per Rail Center's `DenialEventRequest`.
#[derive(Serialize)]
struct Report<'a> {
    // Absent, never null, where a fallback denied.
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_id: Option<Uuid>,
    // Without its slug, per the contract; null where the call named no endpoint.
    endpoint_key: Option<&'a str>,
    denied_at: String,
    // Rail Center finds the agent and its score from the ticket it stored under this id.
    #[serde(skip_serializing_if = "Option::is_none")]
    ticket_id: Option<Uuid>,
    metadata: Metadata,
}

#[derive(Serialize)]
struct Metadata {
    endpoint_resolution: &'static str,
    // This reader's own reading of the ticket, under the key Rail Center reads it from.
    #[serde(rename = "x-rail-status")]
    ticket_state: &'static str,
    #[serde(
        rename = "claimed-x-rail-status",
        skip_serializing_if = "Option::is_none"
    )]
    claimed_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fallback: Option<FallbackDenial>,
}

// The sink's `send`, its type erased so the reporter isn't generic.
type Post = Arc<
    dyn Fn(Vec<u8>) -> Pin<Box<dyn Future<Output = Result<Response, FetchError>> + Send>>
        + Send
        + Sync,
>;

/// Sends each denial report on its own task, at most 64 at once and 16 of them naming no policy;
/// a report past either cap is dropped and counted. Rail Center gets one attempt at each.
pub struct DenialReporter {
    runtime: Handle,
    post: Post,
    now: WallClock,
    in_flight: Arc<Semaphore>,
    in_flight_naming_no_policy: Arc<Semaphore>,
    dropped: Arc<AtomicU64>,
}

impl DenialReporter {
    /// Reports go to `sink` on `runtime`, each stamped by `now`.
    pub fn new(runtime: &Handle, sink: impl DenialSink, now: WallClock) -> Self {
        let sink = Arc::new(sink);
        let post: Post = Arc::new(move |body| {
            let sink = Arc::clone(&sink);
            Box::pin(async move { sink.send(body).await })
        });
        Self {
            runtime: runtime.clone(),
            post,
            now,
            in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
            in_flight_naming_no_policy: Arc::new(Semaphore::new(MAX_IN_FLIGHT_NAMING_NO_POLICY)),
            dropped: Arc::default(),
        }
    }

    /// Starts reporting `denial`, never waiting: the call it reports has been answered either way.
    pub fn report(&self, denial: &Denial) {
        let naming_no_policy = match denial.denied_by {
            DeniedBy::Policy(_) => None,
            DeniedBy::Fallback(_) => {
                match Arc::clone(&self.in_flight_naming_no_policy).try_acquire_owned() {
                    Ok(permit) => Some(permit),
                    Err(_) => return self.drop_report(denial),
                }
            }
        };
        let Ok(permit) = Arc::clone(&self.in_flight).try_acquire_owned() else {
            return self.drop_report(denial);
        };
        // Built once: Rail Center tells reports apart by what they hold, `denied_at` included.
        let Some(body) = build_report(denial, (self.now)()) else {
            tracing::warn!(
                "dropping a denial report: the wall clock is past what RFC 3339 can write"
            );
            return;
        };
        let post = Arc::clone(&self.post);
        let (policy, fallback, endpoint_key) = log_fields(denial);
        let endpoint_key = endpoint_key.map(str::to_owned);
        self.runtime.spawn(async move {
            match read_status(post(body).await, StatusCode::ACCEPTED) {
                Ok(_) => tracing::debug!(
                    policy = policy.map(tracing::field::display),
                    fallback,
                    endpoint_key,
                    "denial reported"
                ),
                Err(reason) => tracing::warn!(
                    reason,
                    policy = policy.map(tracing::field::display),
                    fallback,
                    endpoint_key,
                    "denial report failed; the call stays denied, and Rail Center doesn't list it"
                ),
            }
            drop((permit, naming_no_policy));
        });
    }

    /// How many reports were dropped for being past a cap.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn drop_report(&self, denial: &Denial) {
        let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        let (policy, fallback, endpoint_key) = log_fields(denial);
        tracing::warn!(
            policy = policy.map(tracing::field::display),
            fallback,
            endpoint_key,
            in_flight = MAX_IN_FLIGHT - self.in_flight.available_permits(),
            naming_no_policy = MAX_IN_FLIGHT_NAMING_NO_POLICY
                - self.in_flight_naming_no_policy.available_permits(),
            dropped,
            "dropping a denial report, with too many in flight; the call stays denied, and Rail \
             Center doesn't list it"
        );
    }
}

// The policy or fallback that denied, and the key, for the log.
fn log_fields<'a>(denial: &Denial<'a>) -> (Option<Uuid>, Option<&'static str>, Option<&'a str>) {
    let (policy, fallback) = match denial.denied_by {
        DeniedBy::Policy(policy) => (Some(policy), None),
        DeniedBy::Fallback(fallback) => (None, Some(fallback_name(fallback))),
    };
    (policy, fallback, endpoint_key(denial.endpoint))
}

// The report's JSON, or `None` if `now` can't be written.
fn build_report(denial: &Denial, now: std::time::SystemTime) -> Option<Vec<u8>> {
    let denied_at = OffsetDateTime::from(now).format(&Rfc3339).ok()?;
    let (policy_id, fallback) = match denial.denied_by {
        DeniedBy::Policy(policy) => (Some(policy), None),
        DeniedBy::Fallback(fallback) => (None, Some(fallback)),
    };
    let report = Report {
        policy_id,
        endpoint_key: endpoint_key(denial.endpoint),
        denied_at,
        ticket_id: ticket_id(denial.ticket),
        metadata: Metadata {
            endpoint_resolution: match denial.endpoint {
                Endpoint::Keyless => "keyless",
                Endpoint::Unrecognised => "unrecognised",
                Endpoint::Resolved(_) => "resolved",
            },
            ticket_state: ticket_state(denial.ticket),
            claimed_status: denial
                .claimed_status
                .map(|claimed| claimed.chars().take(MAX_CLAIMED_STATUS_CHARS).collect()),
            fallback,
        },
    };
    Some(serde_json::to_vec(&report).expect("a report has only string keys"))
}

fn endpoint_key(endpoint: &Endpoint) -> Option<&str> {
    match endpoint {
        Endpoint::Resolved(key) => Some(key.as_str()),
        Endpoint::Keyless | Endpoint::Unrecognised => None,
    }
}

// Only a usable ticket's, and only a uuid, which Rail Center reads: any other would refuse the
// whole report.
fn ticket_id(ticket: &TicketState) -> Option<Uuid> {
    let TicketState::Valid(ticket) = ticket else {
        return None;
    };
    Uuid::parse_str(ticket.claims.get("id")?.as_str()?).ok()
}

fn ticket_state(ticket: &TicketState) -> &'static str {
    match ticket {
        TicketState::Absent => "absent",
        TicketState::Undecodable => "undecodable",
        TicketState::BadSignature => "bad_signature",
        TicketState::Malformed => "malformed",
        TicketState::WrongAudience => "wrong_audience",
        TicketState::Expired => "expired",
        TicketState::Valid(_) => "valid",
    }
}

fn fallback_name(fallback: FallbackDenial) -> &'static str {
    match fallback {
        FallbackDenial::Ticket => "ticket",
        FallbackDenial::Binding => "binding",
    }
}
