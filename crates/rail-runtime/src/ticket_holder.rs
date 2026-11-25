use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rail_model::agent::AgentId;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use time::PrimitiveDateTime;
use time::macros::format_description;
use tokio::runtime::Handle;

use crate::client::{FetchError, Response, Unread, read_ok};
use crate::clock::WallClock;
use crate::config::Secret;
use crate::holder::{Attempt, Holder, Jitter, Kind, Step};
use crate::peek::locate;
use crate::ticket::{OpaqueTicket, TicketSource};
use crate::xrail::Unavailable;

// What the answers so far leave to present.
#[derive(Default)]
enum Held {
    // No answer yet, or the last a failure or a refusal other than "none", with no ticket held.
    #[default]
    Unreached,
    // Set only by Rail Center answering "none", never by a failed fetch.
    NotFound,
    Ticket(Arc<OpaqueTicket>),
}

impl Held {
    fn ticket(&self) -> Option<&Arc<OpaqueTicket>> {
        match self {
            Self::Ticket(ticket) => Some(ticket),
            Self::Unreached | Self::NotFound => None,
        }
    }
}

/// Keeps a valid ticket to hand through Rail Center failures; dropping it stops the refreshes.
pub struct TicketHolder {
    holder: Holder<Held>,
    now: WallClock,
}

impl TicketHolder {
    /// Starts fetching at once, on `runtime`, and again every `refresh_interval`, or at half the
    /// ticket's remaining life if sooner; anything but a valid ticket backs off from 5 s up to 320 s
    /// or the interval. `jitter` scales every wait.
    pub fn start(
        runtime: &Handle,
        source: impl TicketSource,
        refresh_interval: Duration,
        now: WallClock,
        jitter: Jitter,
    ) -> Self {
        let kind = Tickets {
            source,
            now: Arc::clone(&now),
        };
        let holder = Holder::start(runtime, kind, Held::default(), refresh_interval, jitter);
        Self { holder, now }
    }

    /// The ticket to present, or why there is none, from one read of the ticket and the clock.
    pub fn current(&self) -> Result<Arc<OpaqueTicket>, Unavailable> {
        match &*self.holder.snapshot() {
            Held::Ticket(ticket) if ticket.is_valid((self.now)()) => Ok(Arc::clone(ticket)),
            Held::Ticket(_) => Err(Unavailable::Expired),
            Held::NotFound => Err(Unavailable::NotFound),
            Held::Unreached => Err(Unavailable::IssuerUnreachable),
        }
    }
}

struct Tickets<S> {
    source: S,
    now: WallClock,
}

impl<S: TicketSource> Kind for Tickets<S> {
    type Held = Held;
    // When the fetch was sent, and what it brought.
    type Answer = (SystemTime, Result<Response, FetchError>);
    type Outcome = Outcome;

    const WHAT: &'static str = "ticket";
    const EXPIRES: bool = true;

    fn fetch(&self) -> impl Future<Output = Self::Answer> + Send {
        let sent_at = (self.now)();
        let fetch = self.source.fetch();
        async move { (sent_at, fetch.await) }
    }

    fn step(&self, held: &Held, (sent_at, answer): Self::Answer) -> Step<Held, Outcome> {
        let answer = read_ticket(answer, self.source.agent(), sent_at);
        step(held, answer, (self.now)())
    }

    // Half the remaining life, so a short ticket is replaced before it lapses.
    fn wait_at_most(&self, held: &Held) -> Option<Duration> {
        held.ticket()
            .and_then(|ticket| ticket.expires_at.duration_since((self.now)()).ok())
            .map(|remaining| remaining / 2)
    }

    fn report(&self, outcome: Outcome, held: &Held, _: bool, retry: u64) {
        log(
            &outcome,
            held.ticket().map(|ticket| &**ticket),
            (self.now)(),
            retry,
        );
    }
}

// What one answer, read at `at`, makes of the ticket held. Only a valid ticket is a success.
fn step(
    previous: &Held,
    answer: Result<TicketAnswer, Unread>,
    at: SystemTime,
) -> Step<Held, Outcome> {
    let usable = |held: &Held| held.ticket().is_some_and(|ticket| ticket.is_valid(at));
    let (next, attempt, outcome) = match answer {
        Ok(TicketAnswer::NoTicket) => (
            Some(Held::NotFound),
            Attempt::Refused,
            Outcome::NoTicket {
                cleared: usable(previous),
            },
        ),
        Err(unread) => {
            let kept = usable(previous);
            // A ticket held is kept; only "none" gives way to "unreached".
            let next = previous.ticket().is_none().then_some(Held::Unreached);
            let attempt = Attempt::of(&unread);
            let reason = unread.reason;
            (next, attempt, Outcome::Failed { reason, kept })
        }
        // An already-lapsed ticket never displaces one still good.
        Ok(TicketAnswer::Ticket(ticket)) if !ticket.is_valid(at) && usable(previous) => {
            let lapsed = fingerprint(ticket.token.expose());
            (None, Attempt::Refused, Outcome::LapsedKept { lapsed })
        }
        Ok(TicketAnswer::Ticket(ticket)) => {
            let valid = ticket.is_valid(at);
            let outcome = match previous.ticket() {
                _ if !valid => Outcome::Lapsed,
                None => Outcome::Acquired,
                Some(held) if held.token != ticket.token => Outcome::Rotated,
                Some(_) => Outcome::Unchanged,
            };
            let attempt = if valid {
                Attempt::Succeeded
            } else {
                Attempt::Refused
            };
            (Some(Held::Ticket(Arc::new(ticket))), attempt, outcome)
        }
    };
    Step {
        next,
        attempt,
        outcome,
    }
}

// What one fetch did to the ticket held, for the log.
enum Outcome {
    Acquired,
    Rotated,
    Unchanged,
    NoTicket { cleared: bool },
    Failed { reason: String, kept: bool },
    // Not the one held, which is kept.
    LapsedKept { lapsed: String },
    Lapsed,
}

fn log(outcome: &Outcome, ticket: Option<&OpaqueTicket>, now: SystemTime, retry: u64) {
    let fingerprint = ticket.map(|ticket| fingerprint(ticket.token.expose()));
    let fingerprint = fingerprint.as_deref().unwrap_or("none");
    let expires_in = ticket
        .and_then(|ticket| ticket.expires_at.duration_since(now).ok())
        .unwrap_or_default()
        .as_secs();
    match outcome {
        Outcome::Acquired => {
            tracing::info!(fingerprint, expires_in, "ticket acquired");
        }
        Outcome::Rotated => {
            tracing::info!(fingerprint, expires_in, "ticket rotated");
        }
        Outcome::Unchanged => {
            tracing::debug!(fingerprint, expires_in, "ticket unchanged");
        }
        Outcome::NoTicket { cleared } => {
            tracing::warn!(
                cleared,
                retry,
                "Rail Center holds no ticket for this host and sandbox"
            );
        }
        Outcome::Failed { reason, kept: true } => {
            tracing::warn!(
                reason,
                fingerprint,
                expires_in,
                retry,
                "ticket fetch failed; keeping the one held"
            );
        }
        Outcome::Failed {
            reason,
            kept: false,
        } => {
            tracing::warn!(reason, retry, "ticket fetch failed; none is held");
        }
        Outcome::LapsedKept { lapsed } => {
            tracing::warn!(
                lapsed,
                fingerprint,
                expires_in,
                "Rail Center returned a lapsed ticket; keeping the one held"
            );
        }
        Outcome::Lapsed => {
            tracing::warn!(retry, "Rail Center returned a lapsed ticket; none is valid");
        }
    }
}

/// A short label for a ticket: shows that it rotated, never what it is.
fn fingerprint(token: &str) -> String {
    Sha256::digest(token.as_bytes())[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// Rail Center stamps at most a day (`TICKET_TTL_MAX_SECONDS`), plus a margin for skew.
const MAX_LIFETIME: Duration = Duration::from_secs(86_400 + 300);

/// What Rail Center answered: a ticket, or that none exists for this host and sandbox.
enum TicketAnswer {
    Ticket(OpaqueTicket),
    NoTicket,
}

#[derive(Deserialize)]
struct Envelope {
    host_id: String,
    tickets: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    sandbox_name: Option<String>,
    token: String,
    expires_at: String,
}

/// The ticket an answer brings for `agent`, or why none; `sent_at`, read before the fetch, keeps
/// the round trip off its life.
fn read_ticket(
    answer: Result<Response, FetchError>,
    agent: &AgentId,
    sent_at: SystemTime,
) -> Result<TicketAnswer, Unread> {
    let body = read_ok(answer)?;
    read_body(&body, agent, sent_at).map_err(Unread::refused)
}

fn read_body(body: &[u8], agent: &AgentId, sent_at: SystemTime) -> Result<TicketAnswer, String> {
    let envelope: Envelope = serde_json::from_slice(body)
        .map_err(|error| format!("the response is off the contract ({})", locate(&error)))?;
    if envelope.host_id != agent.host_id {
        return Err(format!(
            "the response answers for host {}, not {}",
            envelope.host_id, agent.host_id
        ));
    }
    let entry = match <[Entry; 1]>::try_from(envelope.tickets) {
        Ok([entry]) => entry,
        Err(tickets) if tickets.is_empty() => return Ok(TicketAnswer::NoTicket),
        Err(tickets) => {
            return Err(format!(
                "{} tickets for one sandbox; the narrowing wasn't honoured",
                tickets.len()
            ));
        }
    };
    // An unnamed or other sandbox's entry is another agent's identity.
    if entry.sandbox_name.as_deref() != Some(agent.sandbox_name.as_str()) {
        return Err(format!(
            "the ticket names sandbox {}, not {}",
            entry.sandbox_name.as_deref().unwrap_or("none"),
            agent.sandbox_name
        ));
    }
    check_token(&entry.token)?;
    let expires_at = parse_expires_at(&entry.expires_at)?;
    if expires_at
        .duration_since(sent_at)
        .is_ok_and(|lifetime| lifetime > MAX_LIFETIME)
    {
        return Err(format!(
            "the ticket expires more than {} s away, past what Rail Center stamps",
            MAX_LIFETIME.as_secs()
        ));
    }
    Ok(TicketAnswer::Ticket(OpaqueTicket {
        token: Secret::new(entry.token),
        expires_at,
    }))
}

// base64url, padded or not, and a JWT's dots: what Rail Center mints. Never quoted.
fn check_token(token: &str) -> Result<(), String> {
    let valid =
        |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'=');
    if token.is_empty() || !token.bytes().all(valid) {
        return Err(format!(
            "the ticket isn't base64url ({} characters)",
            token.chars().count()
        ));
    }
    Ok(())
}

/// Reads `expires_at` as Rail Center writes it, `2026-07-28T10:15:00Z`, and nothing else.
fn parse_expires_at(text: &str) -> Result<SystemTime, String> {
    let format = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    PrimitiveDateTime::parse(text, &format)
        .map(|at| at.assume_utc().into())
        .map_err(|_| format!("expires_at isn't YYYY-MM-DDTHH:MM:SSZ: {text}"))
}
