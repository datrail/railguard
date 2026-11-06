use std::collections::{HashMap, HashSet};

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
    pub exp_epoch_seconds: i64,
    /// The payload as sent, which rules read. Only `exp`, and on a signed ticket `nbf`, `iss` and
    /// `aud`, is checked, so Rail Center can add or drop any other claim.
    pub claims: Map<String, Value>,
}

/// What a reader needs to accept a signed ticket.
#[derive(Debug)]
pub struct SignedTicketConfig {
    /// By `kid`.
    pub keys: HashMap<String, VerifyingKey>,
    /// `None` or empty where not configured, which accepts no signed ticket.
    pub issuer: Option<String>,
    /// As `issuer`.
    pub audience: Option<String>,
}

/// Reads either kind of ticket. A `.` is outside both base64 alphabets, so only a signed ticket
/// holds one. The caller handles the header as for `parse_base64_ticket`.
pub fn parse_ticket(
    value: &str,
    config: &SignedTicketConfig,
    now_epoch_seconds: i64,
) -> TicketState {
    if value.contains('.') {
        return parse_jwt_ticket(value, config, now_epoch_seconds);
    }
    parse_base64_ticket(value, now_epoch_seconds)
}

/// Reads an unsigned ticket. The caller reads a missing header as `Absent`, and a repeated one
/// as `Undecodable`.
pub fn parse_base64_ticket(value: &str, now_epoch_seconds: i64) -> TicketState {
    if value.is_empty() {
        return TicketState::Absent;
    }
    let Some(payload) = decode_base64(value).and_then(|bytes| read_json(&bytes)) else {
        return TicketState::Undecodable;
    };
    let Value::Object(claims) = payload else {
        return TicketState::Malformed;
    };
    let Some(exp_epoch_seconds) = claims.get("exp").and_then(read_seconds) else {
        return TicketState::Malformed;
    };
    check_expiry(claims, exp_epoch_seconds, now_epoch_seconds)
}

/// Reads a signed ticket: first failure wins, and nothing in the payload is read until the
/// signature holds. The caller handles the header as for `parse_base64_ticket`.
pub fn parse_jwt_ticket(
    value: &str,
    config: &SignedTicketConfig,
    now_epoch_seconds: i64,
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
    let Some(key) = header["kid"].as_str().and_then(|kid| config.keys.get(kid)) else {
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
    let (Some(exp_epoch_seconds), Some(nbf_epoch_seconds)) = (
        claims.get("exp").and_then(read_seconds),
        claims.get("nbf").and_then(read_seconds),
    ) else {
        return TicketState::Malformed;
    };

    // Genuine, so not `BadSignature`, but with nothing to compare against it isn't for us either.
    // An empty one would match a ticket carrying `iss: ""`.
    let issuer = config.issuer.as_deref().filter(|issuer| !issuer.is_empty());
    let audience = config
        .audience
        .as_deref()
        .filter(|audience| !audience.is_empty());
    let (Some(issuer), Some(audience)) = (issuer, audience) else {
        return TicketState::WrongAudience;
    };
    if now_epoch_seconds < nbf_epoch_seconds - NBF_ALLOWANCE_IN_SECONDS
        || claims.get("iss").and_then(Value::as_str) != Some(issuer)
        || claims.get("aud").and_then(Value::as_str) != Some(audience)
    {
        return TicketState::WrongAudience;
    }
    check_expiry(claims, exp_epoch_seconds, now_epoch_seconds)
}

/// A key set as read: the keys to verify signed tickets with, and the entries meant as keys that
/// can't be.
#[derive(Debug, PartialEq)]
pub struct ParsedKeySet {
    /// By `kid`; empty where no entry is usable, which a reader counts as a failed fetch.
    pub keys: HashMap<String, VerifyingKey>,
    pub dropped: Vec<DroppedKey>,
}

/// An entry that claims to be an Ed25519 signing key, and isn't usable as one.
#[derive(Debug, PartialEq)]
pub struct DroppedKey {
    /// Its `kid` as sent: the string, or the JSON of anything else.
    pub kid: Option<String>,
    pub reason: &'static str,
}

/// A JWKS as Rail Center publishes it: `None` where it isn't an object of `keys`, or two entries
/// share a `kid`.
///
/// A usable key is `OKP` `Ed25519`, with a string `kid` and an `x` of 32 bytes on the curve. An
/// entry whose `use`, `key_ops` or `alg` is for something else is skipped without a word, so Rail
/// Center can publish keys for other purposes; one for signing that isn't usable is dropped.
pub fn parse_key_set(body: &[u8]) -> Option<ParsedKeySet> {
    let set = read_json(body)?;
    let entries = set.get("keys")?.as_array()?;
    let mut kids = HashSet::new();
    let mut read = ParsedKeySet {
        keys: HashMap::new(),
        dropped: Vec::new(),
    };
    for entry in entries.iter().filter_map(Value::as_object) {
        // Over every entry, usable or not: a ticket naming a shared `kid` can't say which it meant.
        // Each as its JSON text, and a `null` one as none, as a reader on another runtime sees it.
        let kid = entry.get("kid").filter(|kid| !kid.is_null());
        if !kids.insert(kid.map(Value::to_string)) {
            return None;
        }
        match read_key(entry) {
            Entry::Other => {}
            Entry::Dropped(dropped) => read.dropped.push(dropped),
            Entry::Key(kid, key) => {
                read.keys.insert(kid, key);
            }
        }
    }
    Some(read)
}

enum Entry {
    // For another purpose.
    Other,
    Dropped(DroppedKey),
    Key(String, VerifyingKey),
}

fn read_key(entry: &Map<String, Value>) -> Entry {
    let is = |name: &str, value: &str| entry.get(name).and_then(Value::as_str) == Some(value);
    let absent_or = |name: &str, value: &str| !entry.contains_key(name) || is(name, value);
    let verifies = entry.get("key_ops").is_none_or(|ops| {
        ops.as_array()
            .is_some_and(|ops| ops.iter().any(|op| op.as_str() == Some("verify")))
    });
    let for_verifying = is("kty", "OKP")
        && is("crv", "Ed25519")
        && absent_or("use", "sig")
        && absent_or("alg", "EdDSA")
        && verifies;
    if !for_verifying {
        return Entry::Other;
    }
    let drop = |reason| {
        let kid = entry.get("kid").map(|kid| match kid {
            Value::String(kid) => kid.clone(),
            kid => kid.to_string(),
        });
        Entry::Dropped(DroppedKey { kid, reason })
    };
    let Some(kid) = entry.get("kid").and_then(Value::as_str) else {
        return drop("its kid isn't a string");
    };
    let Some(x) = entry.get("x").and_then(Value::as_str) else {
        return drop("its x isn't a string");
    };
    let Some(x) = decode_base64(x) else {
        return drop("its x isn't base64");
    };
    let Ok(x) = <[u8; 32]>::try_from(x) else {
        return drop("its x isn't 32 bytes");
    };
    match VerifyingKey::from_bytes(&x) {
        Ok(key) => Entry::Key(kid.to_owned(), key),
        Err(_) => drop("its x isn't a point on the Ed25519 curve"),
    }
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
    exp_epoch_seconds: i64,
    now_epoch_seconds: i64,
) -> TicketState {
    if now_epoch_seconds >= exp_epoch_seconds {
        return TicketState::Expired;
    }
    TicketState::Valid(Box::new(Ticket {
        exp_epoch_seconds,
        claims,
    }))
}
