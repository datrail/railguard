use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE;
use serde_json::Value;

/// How an `x-rail` value classifies. Only `Valid` carries claims.
#[derive(Debug, PartialEq)]
pub enum TicketState {
    Absent,
    Undecodable,
    BadSignature,
    Malformed,
    WrongAudience,
    Expired,
    Valid(Ticket),
}

#[derive(Debug, PartialEq)]
pub struct Ticket {
    /// The ticket is dead at this instant, since the epoch.
    pub exp_in_seconds: i64,
    /// Conditions walk these by path, so no claim is known by name here.
    pub claims: Value,
}

/// What a reader needs to accept a signed ticket.
#[derive(Debug)]
pub struct TicketVerifier {
    /// Ed25519 public keys by `kid`.
    pub keys: HashMap<String, [u8; 32]>,
    pub issuer: String,
    pub audience: String,
}

/// Reads an unsigned ticket.
pub fn parse_base64_ticket(value: &str, now_in_seconds: i64) -> TicketState {
    let Ok(bytes) = URL_SAFE.decode(value) else {
        return TicketState::Undecodable;
    };
    let Ok(claims) = serde_json::from_slice::<Value>(&bytes) else {
        return TicketState::Undecodable;
    };
    // JSON that isn't an object has no `exp` either.
    let Some(exp_in_seconds) = claims["exp"].as_i64() else {
        return TicketState::Malformed;
    };
    if now_in_seconds >= exp_in_seconds {
        return TicketState::Expired;
    }
    TicketState::Valid(Ticket {
        exp_in_seconds,
        claims,
    })
}

/// Reads a signed ticket.
pub fn parse_jwt_ticket(
    _value: &str,
    _verifier: &TicketVerifier,
    _now_in_seconds: i64,
) -> TicketState {
    todo!()
}
