use std::collections::{BTreeMap, HashMap};

use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

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
    pub claims: TicketClaims,
}

/// The claims rules can read. A signed ticket's extra claims are only used to verify it.
#[derive(Debug, PartialEq, Deserialize)]
pub struct TicketClaims {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub sandbox_id: String,
    pub sandbox_type: Option<String>,
    pub environment_fingerprint: String,
    pub skills: Vec<String>,
    pub posture: Posture,
    pub alignment: Alignment,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct Posture {
    pub overall: OverallPosture,
    pub categories: BTreeMap<String, CategoryPosture>,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct OverallPosture {
    pub score: f64,
    pub level: PostureLevel,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PostureLevel {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct CategoryPosture {
    /// `None` where the category wasn't scored.
    pub score: Option<f64>,
    pub capped: bool,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct Alignment {
    /// `None` for an agent on no baseline.
    pub baseline: Option<Baseline>,
    pub drifted: Vec<String>,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct Baseline {
    pub id: Uuid,
    pub label: String,
}

/// What a reader needs to accept a signed ticket.
#[derive(Debug)]
pub struct TicketVerifier {
    /// By `kid`.
    pub keys: HashMap<String, VerifyingKey>,
    pub issuer: String,
    pub audience: String,
}

/// Reads an unsigned ticket.
pub fn parse_base64_ticket(value: &str, now_in_seconds: i64) -> TicketState {
    let Ok(bytes) = URL_SAFE.decode(value) else {
        return TicketState::Undecodable;
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&bytes) else {
        return TicketState::Undecodable;
    };
    // JSON that isn't an object has no `exp` either.
    let Some(exp_in_seconds) = payload["exp"].as_i64() else {
        return TicketState::Malformed;
    };
    let Ok(claims) = TicketClaims::deserialize(&payload) else {
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
    let (Some(exp_in_seconds), Some(nbf_in_seconds)) =
        (payload["exp"].as_i64(), payload["nbf"].as_i64())
    else {
        return TicketState::Malformed;
    };
    let Ok(claims) = TicketClaims::deserialize(&payload) else {
        return TicketState::Malformed;
    };

    if now_in_seconds < nbf_in_seconds.saturating_sub(NBF_ALLOWANCE_IN_SECONDS)
        || payload["iss"].as_str() != Some(&verifier.issuer)
        || payload["aud"].as_str() != Some(&verifier.audience)
    {
        return TicketState::WrongAudience;
    }
    check_expiry(claims, exp_in_seconds, now_in_seconds)
}

fn check_expiry(claims: TicketClaims, exp_in_seconds: i64, now_in_seconds: i64) -> TicketState {
    if now_in_seconds >= exp_in_seconds {
        return TicketState::Expired;
    }
    TicketState::Valid(Box::new(Ticket {
        exp_in_seconds,
        claims,
    }))
}
