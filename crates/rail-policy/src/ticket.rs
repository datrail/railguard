use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::{Map, Value};

/// A reader's clock may lag Rail Center's, so a fresh ticket's `nbf` gets this much. `exp` gets none.
const NBF_ALLOWANCE_IN_SECONDS: i64 = 60;

/// How an `x-rail` value classifies. Only `Valid` carries claims.
#[derive(Debug, PartialEq)]
pub enum TicketState {
    Absent,
    Undecodable,
    BadSignature,
    Malformed,
    WrongAudience,
    Expired,
    Valid(Box<Ticket>),
}

#[derive(Debug, PartialEq)]
pub struct Ticket {
    /// The ticket is dead at this instant, since the epoch.
    pub exp_in_seconds: i64,
    /// The payload as sent, which rules read. Only `exp`, and on a signed ticket `nbf`, `iss` and
    /// `aud`, is checked, so Rail Center can add or drop any other claim.
    pub claims: Map<String, Value>,
}

/// What a reader needs to accept a signed ticket.
#[derive(Debug)]
pub struct TicketVerifier {
    /// By `kid`.
    pub keys: HashMap<String, VerifyingKey>,
    /// `None` where not configured, which accepts no signed ticket. Never `Some("")`: it would
    /// match a ticket carrying `iss: ""`.
    pub issuer: Option<String>,
    /// As `issuer`.
    pub audience: Option<String>,
}

/// Reads an unsigned ticket.
pub fn parse_base64_ticket(value: &str, now_in_seconds: i64) -> TicketState {
    let Ok(bytes) = URL_SAFE.decode(value) else {
        return TicketState::Undecodable;
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&bytes) else {
        return TicketState::Undecodable;
    };
    let Value::Object(claims) = payload else {
        return TicketState::Malformed;
    };
    let Some(exp_in_seconds) = claims.get("exp").and_then(Value::as_i64) else {
        return TicketState::Malformed;
    };
    check_expiry(claims, exp_in_seconds, now_in_seconds)
}

/// Reads a signed ticket: first failure wins, and nothing in the payload is read until the
/// signature holds.
pub fn parse_jwt_ticket(
    value: &str,
    verifier: &TicketVerifier,
    now_in_seconds: i64,
) -> TicketState {
    let parts: Vec<&str> = value.split('.').collect();
    let [header, payload, signature] = parts[..] else {
        return TicketState::Undecodable;
    };
    if parts.contains(&"") {
        return TicketState::Undecodable;
    }
    // The signature covers the first two parts exactly as received.
    let signed = &value[..value.len() - signature.len() - 1];
    let (Ok(header), Ok(payload), Ok(signature)) = (
        URL_SAFE_NO_PAD.decode(header),
        URL_SAFE_NO_PAD.decode(payload),
        URL_SAFE_NO_PAD.decode(signature),
    ) else {
        return TicketState::Undecodable;
    };
    let Ok(header @ Value::Object(_)) = serde_json::from_slice(&header) else {
        return TicketState::Undecodable;
    };

    // Always Ed25519. If a ticket could choose its algorithm, a forged one could choose `none`.
    if header["alg"] != "EdDSA" || header.get("crit").is_some() {
        return TicketState::BadSignature;
    }
    let Some(key) = header["kid"]
        .as_str()
        .and_then(|kid| verifier.keys.get(kid))
    else {
        return TicketState::BadSignature;
    };
    let Ok(signature) = Signature::from_slice(&signature) else {
        return TicketState::BadSignature;
    };
    if key.verify(signed.as_bytes(), &signature).is_err() {
        return TicketState::BadSignature;
    }

    let Ok(payload) = serde_json::from_slice::<Value>(&payload) else {
        return TicketState::Undecodable;
    };
    let Value::Object(claims) = payload else {
        return TicketState::Malformed;
    };
    let (Some(exp_in_seconds), Some(nbf_in_seconds)) = (
        claims.get("exp").and_then(Value::as_i64),
        claims.get("nbf").and_then(Value::as_i64),
    ) else {
        return TicketState::Malformed;
    };

    // Genuine, so not `BadSignature`, but with nothing to compare against it isn't for us either.
    let (Some(issuer), Some(audience)) = (&verifier.issuer, &verifier.audience) else {
        return TicketState::WrongAudience;
    };
    if now_in_seconds < nbf_in_seconds.saturating_sub(NBF_ALLOWANCE_IN_SECONDS)
        || claims.get("iss").and_then(Value::as_str) != Some(issuer)
        || claims.get("aud").and_then(Value::as_str) != Some(audience)
    {
        return TicketState::WrongAudience;
    }
    check_expiry(claims, exp_in_seconds, now_in_seconds)
}

fn check_expiry(
    claims: Map<String, Value>,
    exp_in_seconds: i64,
    now_in_seconds: i64,
) -> TicketState {
    if now_in_seconds >= exp_in_seconds {
        return TicketState::Expired;
    }
    TicketState::Valid(Box::new(Ticket {
        exp_in_seconds,
        claims,
    }))
}
