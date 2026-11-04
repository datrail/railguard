use std::collections::HashMap;

use base64::Engine;
use base64::alphabet::URL_SAFE;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::{Map, Value};

/// A reader's clock may lag Rail Center's, so a fresh ticket's `nbf` gets this much. `exp` gets none.
const NBF_ALLOWANCE_IN_SECONDS: i64 = 60;

/// The largest whole number a double holds exactly, and so the bound on `exp` and `nbf`.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Decodes a body already stripped of its padding and mapped to the url-safe alphabet.
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::RequireNone)
        .with_decode_allow_trailing_bits(true),
);

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
    /// `None` or empty where not configured, which accepts no signed ticket.
    pub issuer: Option<String>,
    /// As `issuer`.
    pub audience: Option<String>,
}

/// Reads either kind of ticket. A `.` is outside both base64 alphabets, so only a signed ticket
/// holds one. The caller handles the header as for `parse_base64_ticket`.
pub fn parse_ticket(value: &str, verifier: &TicketVerifier, now_in_seconds: i64) -> TicketState {
    if value.contains('.') {
        return parse_jwt_ticket(value, verifier, now_in_seconds);
    }
    parse_base64_ticket(value, now_in_seconds)
}

/// Reads an unsigned ticket. The caller reads a missing header as `Absent`, and a repeated one
/// as `Undecodable`.
pub fn parse_base64_ticket(value: &str, now_in_seconds: i64) -> TicketState {
    if value.is_empty() {
        return TicketState::Absent;
    }
    let Some(payload) = decode_base64(value).and_then(|bytes| read_json(&bytes)) else {
        return TicketState::Undecodable;
    };
    let Value::Object(claims) = payload else {
        return TicketState::Malformed;
    };
    let Some(exp_in_seconds) = claims.get("exp").and_then(read_seconds) else {
        return TicketState::Malformed;
    };
    check_expiry(claims, exp_in_seconds, now_in_seconds)
}

/// Reads a signed ticket: first failure wins, and nothing in the payload is read until the
/// signature holds. The caller handles the header as for `parse_base64_ticket`.
pub fn parse_jwt_ticket(
    value: &str,
    verifier: &TicketVerifier,
    now_in_seconds: i64,
) -> TicketState {
    if value.is_empty() {
        return TicketState::Absent;
    }
    let parts: Vec<&str> = value.split('.').collect();
    let [header, payload, signature] = parts[..] else {
        return TicketState::Undecodable;
    };
    if parts.contains(&"") {
        return TicketState::Undecodable;
    }
    // The signature covers the first two parts exactly as received.
    let signed = &value[..value.len() - signature.len() - 1];
    let (Some(header), Some(payload), Some(signature)) = (
        decode_base64(header),
        decode_base64(payload),
        decode_base64(signature),
    ) else {
        return TicketState::Undecodable;
    };
    let Some(header @ Value::Object(_)) = read_json(&header) else {
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
    // Strict, so a weak key in the set can't be used to forge a ticket.
    if key.verify_strict(signed.as_bytes(), &signature).is_err() {
        return TicketState::BadSignature;
    }

    let Some(payload) = read_json(&payload) else {
        return TicketState::Undecodable;
    };
    let Value::Object(claims) = payload else {
        return TicketState::Malformed;
    };
    let (Some(exp_in_seconds), Some(nbf_in_seconds)) = (
        claims.get("exp").and_then(read_seconds),
        claims.get("nbf").and_then(read_seconds),
    ) else {
        return TicketState::Malformed;
    };

    // Genuine, so not `BadSignature`, but with nothing to compare against it isn't for us either.
    // An empty one would match a ticket carrying `iss: ""`.
    let issuer = verifier
        .issuer
        .as_deref()
        .filter(|issuer| !issuer.is_empty());
    let audience = verifier
        .audience
        .as_deref()
        .filter(|audience| !audience.is_empty());
    let (Some(issuer), Some(audience)) = (issuer, audience) else {
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

/// Either alphabet, padded or not: a trailing run of `=` is dropped, and nothing else is.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    // The engine refuses any other byte, and a body of length 4n + 1.
    let body: String = text
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    BASE64.decode(body).ok()
}

/// Strict UTF-8, less one leading byte-order mark.
fn read_json(bytes: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    serde_json::from_str(text).ok()
}

/// A whole number of seconds a double holds exactly, written with a fraction or without.
fn read_seconds(value: &Value) -> Option<i64> {
    let seconds = value.as_f64()?;
    if seconds.fract() != 0.0 || seconds.abs() > MAX_SAFE_INTEGER {
        return None;
    }
    Some(seconds as i64)
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
