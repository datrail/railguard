use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_policy::sanitize;
use reqwest::StatusCode;
use serde::Deserialize;

use crate::client::{Limits, RailCenterClient};
use crate::config::Secret;

const ROUTE: &str = "/v1/tickets";

/// A named fetch answers with one ticket at most, so a body past this is a wrong answer.
pub const TICKET_LIMITS: Limits = Limits {
    attempt_timeout: Duration::from_secs(10),
    deadline: Duration::from_secs(10),
    max_bytes: 256 * 1024,
};

// Rail Center stamps at most a day (`TICKET_TTL_MAX_SECONDS`); the margin is for clock skew.
const MAX_LIFETIME: Duration = Duration::from_secs(86_400 + 300);

/// Whose ticket to fetch: this host, and the one sandbox it serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TicketQuery {
    pub host_id: String,
    pub sandbox_name: String,
}

/// A ticket to present, opaque, and the instant it stops being valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub token: Secret,
    pub expires_at: SystemTime,
}

impl Ticket {
    pub fn is_valid(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }
}

/// What Rail Center answered: a ticket, or that none exists for this host and sandbox.
#[derive(Debug, PartialEq, Eq)]
pub enum TicketAnswer {
    Ticket(Ticket),
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

/// Fetches this host's ticket. `sent_at` is the wall-clock time read before the call, so the
/// lifetime isn't credited with the round trip. An error is a failed fetch, never "no ticket".
pub async fn fetch_ticket(
    client: &RailCenterClient,
    query: &TicketQuery,
    sent_at: SystemTime,
) -> Result<TicketAnswer, String> {
    let params = [
        ("host_id", query.host_id.as_str()),
        ("sandbox_name", query.sandbox_name.as_str()),
    ];
    let response = client
        .get(ROUTE, &params, TICKET_LIMITS)
        .await
        .map_err(|error| error.to_string())?;
    if response.status != StatusCode::OK {
        return Err(format!("Rail Center answered {}", response.status));
    }
    let envelope: Envelope = serde_json::from_slice(&response.body).map_err(|error| {
        format!(
            "the response is off the contract: {}",
            sanitize(&error.to_string())
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
        .is_ok_and(|lifetime| lifetime > MAX_LIFETIME)
    {
        return Err(format!(
            "the ticket expires more than {} s away, past what Rail Center stamps",
            MAX_LIFETIME.as_secs()
        ));
    }
    Ok(TicketAnswer::Ticket(Ticket {
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

/// Reads Rail Center's `expires_at`: whole seconds in UTC, `2026-07-28T10:15:00Z`, and nothing
/// else.
fn parse_expires_at(text: &str) -> Result<SystemTime, String> {
    let refused = || format!("expires_at isn't YYYY-MM-DDTHH:MM:SSZ: {}", sanitize(text));
    let bytes = text.as_bytes();
    if bytes.len() != 20 || [4, 7, 10, 13, 16, 19].map(|at| bytes[at]) != *b"--T::Z" {
        return Err(refused());
    }
    let number = |from: usize, to: usize| {
        let digits = &bytes[from..to];
        digits.iter().all(u8::is_ascii_digit).then(|| {
            digits
                .iter()
                .fold(0, |value, digit| value * 10 + u64::from(digit - b'0'))
        })
    };
    let fields =
        [(0, 4), (5, 7), (8, 10), (11, 13), (14, 16), (17, 19)].map(|(from, to)| number(from, to));
    let [
        Some(year),
        Some(month),
        Some(day),
        Some(hour),
        Some(minute),
        Some(second),
    ] = fields
    else {
        return Err(refused());
    };
    if year < 1970
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(refused());
    }
    let days = days_since_epoch(year, month, day);
    Ok(UNIX_EPOCH + Duration::from_secs(days * 86_400 + hour * 3_600 + minute * 60 + second))
}

fn days_in_month(year: u64, month: u64) -> u64 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// Howard Hinnant's days_from_civil, for years from 1970.
fn days_since_epoch(year: u64, month: u64, day: u64) -> u64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
