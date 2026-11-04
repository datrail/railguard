use std::time::{Duration, SystemTime};

use rail_policy::sanitize;
use reqwest::StatusCode;
use serde::Deserialize;
use time::PrimitiveDateTime;
use time::macros::format_description;

use crate::client::{Limits, RailCenterClient};
use crate::config::{Secret, Settings};
use crate::ticket_holder::MIN_REFRESH_INTERVAL;

const ROUTE: &str = "/v1/tickets";

pub const HOST_ID: &str = "RAIL_HOST_ID";
pub const SANDBOX_NAME: &str = "RAIL_SANDBOX_NAME";
pub const TICKET_REFRESH_SECONDS: &str = "RAIL_TICKET_REFRESH_SECONDS";

// Rail Center's bounds on the query (`get_host_tickets`), in characters.
const MAX_HOST_ID_CHARS: usize = 64;
const MAX_SANDBOX_NAME_CHARS: usize = 255;

/// How this host's ticket is fetched and kept.
#[derive(Clone, Debug)]
pub struct TicketSettings {
    pub query: TicketQuery,
    /// The longest a valid ticket goes unrefreshed; its own lifetime paces refreshes below it.
    pub refresh_interval: Duration,
    /// Not read from the environment yet.
    pub limits: Limits,
    /// Not read from the environment yet.
    pub max_lifetime: Duration,
}

impl TicketSettings {
    pub fn new(query: TicketQuery) -> Self {
        Self {
            query,
            refresh_interval: Duration::from_secs(3600),
            // A named fetch answers with one ticket at most, so a body past this is a wrong one.
            limits: Limits {
                attempt_timeout: Duration::from_secs(10),
                deadline: Duration::from_secs(10),
                max_bytes: 256 * 1024,
            },
            // Rail Center stamps at most a day (`TICKET_TTL_MAX_SECONDS`), plus a margin for skew.
            max_lifetime: Duration::from_secs(86_400 + 300),
        }
    }
}

/// The ticket's settings, every one set judged; the host and sandbox are required if `required`.
pub fn read_ticket_settings(settings: &mut Settings, required: bool) -> Option<TicketSettings> {
    let host_id = settings
        .get(HOST_ID)
        .required(required)
        .max_chars(MAX_HOST_ID_CHARS)
        .text();
    let sandbox_name = settings
        .get(SANDBOX_NAME)
        .required(required)
        .max_chars(MAX_SANDBOX_NAME_CHARS)
        .text();
    let refresh = settings
        .get(TICKET_REFRESH_SECONDS)
        .parsed(|text| match text.parse::<u64>() {
            // The holder never refreshes faster, so a lower interval would be silently raised.
            Ok(seconds) if Duration::from_secs(seconds) >= MIN_REFRESH_INTERVAL => {
                Ok(Duration::from_secs(seconds))
            }
            _ => Err(format!(
                "must be a whole number of seconds, at least {}, got: {}",
                MIN_REFRESH_INTERVAL.as_secs(),
                sanitize(text)
            )),
        });
    let mut ticket = TicketSettings::new(TicketQuery {
        host_id: host_id?.to_owned(),
        sandbox_name: sandbox_name?.to_owned(),
    });
    if settings.get(TICKET_REFRESH_SECONDS).is_set() {
        ticket.refresh_interval = refresh?;
    }
    Some(ticket)
}

/// Whose ticket to fetch: this host, and the one sandbox it serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TicketQuery {
    pub host_id: String,
    pub sandbox_name: String,
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
pub enum TicketAnswer {
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

/// Fetches this host's ticket; `sent_at`, read before the call, keeps the round trip off its life.
pub async fn fetch_ticket(
    client: &RailCenterClient,
    ticket: &TicketSettings,
    sent_at: SystemTime,
) -> Result<TicketAnswer, String> {
    let query = &ticket.query;
    let params = [
        ("host_id", query.host_id.as_str()),
        ("sandbox_name", query.sandbox_name.as_str()),
    ];
    let response = client
        .get(ROUTE, &params, ticket.limits)
        .await
        .map_err(|error| error.to_string())?;
    if response.status != StatusCode::OK {
        return Err(format!("Rail Center answered {}", response.status));
    }
    let envelope: Envelope = serde_json::from_slice(&response.body).map_err(|error| {
        // Where, not what: serde's message can quote a value, and a value may be the ticket.
        format!(
            "the response is off the contract ({:?} error at line {}, column {})",
            error.classify(),
            error.line(),
            error.column()
        )
    })?;
    if envelope.host_id != query.host_id {
        return Err(format!(
            "the response answers for host {}, not {}",
            sanitize(&envelope.host_id),
            sanitize(&query.host_id)
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
    if entry.sandbox_name.as_deref() != Some(query.sandbox_name.as_str()) {
        return Err(format!(
            "the ticket names sandbox {}, not {}",
            sanitize(entry.sandbox_name.as_deref().unwrap_or("none")),
            sanitize(&query.sandbox_name)
        ));
    }
    check_token(&entry.token)?;
    let expires_at = parse_expires_at(&entry.expires_at)?;
    if expires_at
        .duration_since(sent_at)
        .is_ok_and(|lifetime| lifetime > ticket.max_lifetime)
    {
        return Err(format!(
            "the ticket expires more than {} s away, past what Rail Center stamps",
            ticket.max_lifetime.as_secs()
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
        .map_err(|_| format!("expires_at isn't YYYY-MM-DDTHH:MM:SSZ: {}", sanitize(text)))
}
