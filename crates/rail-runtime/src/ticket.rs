use std::future::Future;
use std::time::{Duration, SystemTime};

use rail_policy::sanitize;
use reqwest::StatusCode;
use serde::Deserialize;
use time::PrimitiveDateTime;
use time::macros::format_description;

use crate::client::{Limits, RailCenterClient};
use crate::config::{Refused, Secret, Settings};

const ROUTE: &str = "/v1/tickets";

// Not settings: a lower floor would let a fleet hammer Rail Center.
pub(crate) const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) const HOST_ID: &str = "RAIL_HOST_ID";
pub(crate) const SANDBOX_NAME: &str = "RAIL_SANDBOX_NAME";
pub(crate) const TICKET_REFRESH_SECONDS: &str = "RAIL_TICKET_REFRESH_SECONDS";

// Rail Center's bounds on the query (`get_host_tickets`), in characters.
const MAX_HOST_ID_CHARS: usize = 64;
const MAX_SANDBOX_NAME_CHARS: usize = 255;

const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(3600);

// A named fetch answers with one ticket at most, so a body past this is a wrong one.
const LIMITS: Limits = Limits {
    attempt_timeout: Duration::from_secs(10),
    deadline: Duration::from_secs(10),
    max_bytes: 256 * 1024,
};

// Rail Center stamps at most a day (`TICKET_TTL_MAX_SECONDS`), plus a margin for skew.
const MAX_LIFETIME: Duration = Duration::from_secs(86_400 + 300);

/// How this host's ticket is fetched and kept.
#[derive(Clone, Debug)]
pub struct TicketSettings {
    pub query: TicketQuery,
    /// The longest a valid ticket goes unrefreshed; its own lifetime paces refreshes below it.
    pub refresh_interval: Duration,
}

impl TicketSettings {
    pub fn new(query: TicketQuery) -> Self {
        Self {
            query,
            refresh_interval: DEFAULT_REFRESH_INTERVAL,
        }
    }
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
        })
        .map(|refresh| refresh.unwrap_or(DEFAULT_REFRESH_INTERVAL));
    let (host_id, sandbox_name, refresh_interval) = (host_id?, sandbox_name?, refresh?);
    let ticket = host_id
        .zip(sandbox_name)
        .map(|(host_id, sandbox_name)| TicketSettings {
            refresh_interval,
            ..TicketSettings::new(TicketQuery {
                host_id: host_id.to_owned(),
                sandbox_name: sandbox_name.to_owned(),
            })
        });
    if let Some(ticket) = &ticket {
        settings.describe(HOST_ID, &ticket.query.host_id);
        settings.describe(SANDBOX_NAME, &ticket.query.sandbox_name);
        settings.describe(TICKET_REFRESH_SECONDS, ticket.refresh_interval.as_secs());
    }
    Ok(ticket)
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

/// Where tickets come from: Rail Center, or a test's script.
pub trait TicketSource: Send + Sync + 'static {
    /// One fetch; `sent_at` is the wall-clock time read just before it.
    fn fetch(
        &self,
        sent_at: SystemTime,
    ) -> impl Future<Output = Result<TicketAnswer, String>> + Send;

    /// The longest a valid ticket goes unrefreshed.
    fn refresh_interval(&self) -> Duration;
}

/// This host's ticket, fetched from Rail Center.
pub struct RailCenterTickets {
    pub client: RailCenterClient,
    pub settings: TicketSettings,
}

impl TicketSource for RailCenterTickets {
    /// Fetches this host's ticket; `sent_at`, read before the call, keeps the round trip off its
    /// life.
    async fn fetch(&self, sent_at: SystemTime) -> Result<TicketAnswer, String> {
        let (client, ticket) = (&self.client, &self.settings);
        let query = &ticket.query;
        let params = [
            ("host_id", query.host_id.as_str()),
            ("sandbox_name", query.sandbox_name.as_str()),
        ];
        let response = client
            .get(ROUTE, &params, LIMITS)
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
                envelope.host_id, query.host_id
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
                entry.sandbox_name.as_deref().unwrap_or("none"),
                query.sandbox_name
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

    fn refresh_interval(&self) -> Duration {
        self.settings.refresh_interval
    }
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{LIMITS, MAX_LIFETIME};

    #[test]
    fn the_limits_and_the_lifetime_are_pinned() {
        assert_eq!(LIMITS.deadline, Duration::from_secs(10));
        assert_eq!(LIMITS.attempt_timeout, Duration::from_secs(10));
        assert_eq!(LIMITS.max_bytes, 256 * 1024);
        assert_eq!(MAX_LIFETIME, Duration::from_secs(86_400 + 300));
    }
}
