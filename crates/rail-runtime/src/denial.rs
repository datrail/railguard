//! Denial reports, sent to Rail Center without the call waiting for them.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rail_policy::{Decision, Endpoint, FallbackDenial, Outcome, TicketState, sanitize};
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
#[derive(Clone, Copy, Debug)]
pub enum DeniedBy {
    /// The `block` policy whose condition held.
    Policy(Uuid),
    /// A fallback, naming no policy.
    Fallback(FallbackDenial),
}

impl DeniedBy {
    /// What denies the call in `outcome`: its first fallback denial, else the policy that denied
    /// it. `None` if nothing did, or if the walk was refused, which is never reported. Only an
    /// outcome judged at `enforce` denies: at `observe` the call is forwarded.
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

// A sink with its future boxed, so the reporter holds any sink without being generic.
trait ErasedSink: Send + Sync {
    fn send(
        &self,
        body: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<Response, FetchError>> + Send + '_>>;
}

impl<S: DenialSink> ErasedSink for S {
    fn send(
        &self,
        body: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<Response, FetchError>> + Send + '_>> {
        Box::pin(DenialSink::send(self, body))
    }
}

/// Sends each denial report on its own task, at most 64 at once and 16 of them naming no policy;
/// a report past either cap is dropped and counted. Rail Center gets one attempt at each.
pub struct DenialReporter {
    runtime: Handle,
    sink: Arc<dyn ErasedSink>,
    now: WallClock,
    in_flight: Arc<Semaphore>,
    in_flight_naming_no_policy: Arc<Semaphore>,
    dropped: Arc<AtomicU64>,
}

impl DenialReporter {
    /// Reports go to `sink` on `runtime`, each stamped by `now`.
    pub fn new(runtime: &Handle, sink: impl DenialSink, now: WallClock) -> Self {
        Self {
            runtime: runtime.clone(),
            sink: Arc::new(sink),
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
        let body = build_report(denial, (self.now)());
        let sink = Arc::clone(&self.sink);
        let denied_by = denial.denied_by;
        let endpoint_key = endpoint_key(denial.endpoint).map(str::to_owned);
        self.runtime.spawn(async move {
            match read_status(sink.send(body).await, StatusCode::ACCEPTED) {
                Ok(_) => tracing::debug!(?denied_by, endpoint_key, "denial reported"),
                Err(reason) => tracing::warn!(
                    reason,
                    ?denied_by,
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
        tracing::warn!(
            denied_by = ?denial.denied_by,
            endpoint_key = endpoint_key(denial.endpoint),
            dropped,
            "dropping a denial report, with too many in flight; the call stays denied, and Rail \
             Center doesn't list it"
        );
    }
}

fn build_report(denial: &Denial, now: std::time::SystemTime) -> Vec<u8> {
    let denied_at = OffsetDateTime::from(now)
        .format(&Rfc3339)
        .expect("a wall clock within years 0 to 9999");
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
                // The caller's text, which Rail Center stores unbounded and shows on its dashboard.
                .map(|claimed| sanitize(claimed).to_string()),
            fallback,
        },
    };
    serde_json::to_vec(&report).expect("a report has only string keys")
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

pub(crate) fn ticket_state(ticket: &TicketState) -> &'static str {
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
