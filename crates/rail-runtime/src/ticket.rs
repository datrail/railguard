use std::future::Future;
use std::time::{Duration, SystemTime};

use rail_model::agent::AgentId;
use serde::Deserialize;
use time::PrimitiveDateTime;
use time::macros::format_description;

use crate::client::{FetchError, Limits, RailCenterClient, Response, Unread, read_ok};
use crate::config::{Refused, Secret, Settings};
use crate::holder::parse_refresh_interval;

const ROUTE: &str = "/v1/tickets";

pub(crate) const HOST_ID: &str = "RAIL_HOST_ID";
pub(crate) const SANDBOX_NAME: &str = "RAIL_SANDBOX_NAME";
pub(crate) const TICKET_REFRESH_SECONDS: &str = "RAIL_TICKET_REFRESH_SECONDS";

// Rail Center's bounds on the query (`get_host_tickets`), in characters.
const MAX_HOST_ID_CHARS: usize = 64;
const MAX_SANDBOX_NAME_CHARS: usize = 255;

const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(3600);

// A named fetch answers with one ticket at most, so a body past this is a wrong one.
const LIMITS: Limits = Limits {
    deadline: Duration::from_secs(10),
    silence: Duration::from_secs(5),
    max_bytes: 256 * 1024,
};

// Rail Center stamps at most a day (`TICKET_TTL_MAX_SECONDS`), plus a margin for skew.
const MAX_LIFETIME: Duration = Duration::from_secs(86_400 + 300);

/// How this host's ticket is fetched and kept.
#[derive(Clone, Debug)]
pub struct TicketSettings {
    pub agent: AgentId,
    /// The longest a valid ticket goes unrefreshed, before jitter; its own lifetime paces
    /// refreshes below it.
    pub refresh_interval: Duration,
}

/// The ticket's settings, every one set judged, `None` if the host or sandbox is missing; both
/// are required if `required`, which is whether Rail Center is used. Described only when read
/// whole.
pub fn read_ticket_settings(
    settings: &mut Settings,
    required: bool,
) -> Result<Option<TicketSettings>, Refused> {
    let host_id = settings
        .get(HOST_ID)
        .required_if(required)
        .max_chars(MAX_HOST_ID_CHARS)
        .text();
    let sandbox_name = settings
        .get(SANDBOX_NAME)
        .required_if(required)
        .max_chars(MAX_SANDBOX_NAME_CHARS)
        .text();
    let refresh = settings
        .get(TICKET_REFRESH_SECONDS)
        .parsed(parse_refresh_interval)
        .map(|refresh| refresh.unwrap_or(DEFAULT_REFRESH_INTERVAL));
    let (host_id, sandbox_name, refresh_interval) = (host_id?, sandbox_name?, refresh?);
    let ticket = host_id
        .zip(sandbox_name)
        .map(|(host_id, sandbox_name)| TicketSettings {
            agent: AgentId {
                host_id: host_id.to_owned(),
                sandbox_name: sandbox_name.to_owned(),
            },
            refresh_interval,
        });
    if let Some(ticket) = &ticket {
        settings.describe(HOST_ID, &ticket.agent.host_id);
        settings.describe(SANDBOX_NAME, &ticket.agent.sandbox_name);
        settings.describe(TICKET_REFRESH_SECONDS, ticket.refresh_interval.as_secs());
    }
    Ok(ticket)
}

/// A ticket to present, opaque, and the instant it stops being valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpaqueTicket {
    pub token: Secret,
    pub expires_at: SystemTime,
}

impl OpaqueTicket {
    pub fn is_valid(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }
}

/// What Rail Center answered: a ticket, or that none exists for this host and sandbox.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TicketAnswer {
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

/// Where tickets come from: Rail Center, or a test's script.
pub trait TicketSource: Send + Sync + 'static {
    /// One fetch, whatever its status: what each means is the holder's.
    fn fetch(&self) -> impl Future<Output = Result<Response, FetchError>> + Send;

    /// The agent whose ticket is fetched, which a ticket must name.
    fn agent(&self) -> &AgentId;

    /// The longest a valid ticket goes unrefreshed, before jitter.
    fn refresh_interval(&self) -> Duration;
}

/// This host's ticket, fetched from Rail Center.
pub struct RailCenterTickets {
    pub client: RailCenterClient,
    pub settings: TicketSettings,
}

impl TicketSource for RailCenterTickets {
    async fn fetch(&self) -> Result<Response, FetchError> {
        let agent = &self.settings.agent;
        let params = [
            ("host_id", agent.host_id.as_str()),
            ("sandbox_name", agent.sandbox_name.as_str()),
        ];
        self.client.get(ROUTE, &params, LIMITS).await
    }

    fn agent(&self) -> &AgentId {
        &self.settings.agent
    }

    fn refresh_interval(&self) -> Duration {
        self.settings.refresh_interval
    }
}

/// The ticket an answer brings for `agent`, or why none; `sent_at`, read before the fetch, keeps
/// the round trip off its life.
pub(crate) fn read_ticket(
    answer: Result<Response, FetchError>,
    agent: &AgentId,
    sent_at: SystemTime,
) -> Result<TicketAnswer, Unread> {
    let body = read_ok(answer)?;
    read_body(&body, agent, sent_at).map_err(|reason| Unread {
        reason,
        transient: false,
    })
}

fn read_body(body: &[u8], agent: &AgentId, sent_at: SystemTime) -> Result<TicketAnswer, String> {
    let envelope: Envelope = serde_json::from_slice(body).map_err(|error| {
        // Where, not what: serde's message can quote a value, and a value may be the ticket.
        format!(
            "the response is off the contract ({:?} error at line {}, column {})",
            error.classify(),
            error.line(),
            error.column()
        )
    })?;
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
