use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::VerifyingKey;
use rail_policy::{TicketState, TicketVerifier, parse_base64_ticket, parse_jwt_ticket};
use serde::Deserialize;
use serde_json::{Value, json};

const BASE64_VECTORS: &str =
    include_str!("../../../vectors/parse_base64_ticket_conformance_cases.json");
const JWT_VECTORS: &str = include_str!("../../../vectors/parse_jwt_ticket_conformance_cases.json");

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    header: String,
    now_in_seconds: i64,
    state: String,
    /// The expected payload, where it is valid.
    token: Option<Value>,
    exp_in_seconds: Option<i64>,
    /// Replaces the file's reader, for a JWT case.
    reader: Option<Reader>,
}

#[derive(Deserialize)]
struct Reader {
    holds_keys: bool,
    issuer: Option<String>,
    audience: Option<String>,
}

#[derive(Deserialize)]
struct JwtVectors {
    kid: String,
    public_key: String,
    issuer: String,
    audience: String,
    cases: Vec<Case>,
}

fn get_state_name(state: &TicketState) -> &'static str {
    match state {
        TicketState::Absent => "absent",
        TicketState::Undecodable => "undecodable",
        TicketState::BadSignature => "bad_signature",
        TicketState::Malformed => "malformed",
        TicketState::WrongAudience => "wrong_audience",
        TicketState::Expired => "expired",
        TicketState::Valid(_) => "valid",
    }
}

fn check_case(case: &Case, state: &TicketState) -> Option<String> {
    let name = get_state_name(state);
    if name != case.state {
        return Some(format!("{}: want {}, got {name}", case.name, case.state));
    }
    if let TicketState::Valid(ticket) = state
        && (Some(_read_numbers_as_doubles(Value::Object(
            ticket.claims.clone(),
        ))) != case.token.clone().map(_read_numbers_as_doubles)
            || Some(ticket.exp_in_seconds) != case.exp_in_seconds)
    {
        return Some(format!("{}: wrong ticket", case.name));
    }
    None
}

/// As rules compare them, so `60` and `60.0` are one value.
fn _read_numbers_as_doubles(value: Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64().unwrap()),
        Value::Array(values) => values.into_iter().map(_read_numbers_as_doubles).collect(),
        Value::Object(object) => object
            .into_iter()
            .map(|(key, value)| (key, _read_numbers_as_doubles(value)))
            .collect(),
        value => value,
    }
}

#[test]
fn test_base64_vectors() {
    let vectors: Vectors = serde_json::from_str(BASE64_VECTORS).unwrap();

    let failures: Vec<String> = vectors
        .cases
        .iter()
        .filter_map(|case| {
            let state = parse_base64_ticket(&case.header, case.now_in_seconds);
            check_case(case, &state)
        })
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn test_jwt_vectors() {
    let vectors: JwtVectors = serde_json::from_str(JWT_VECTORS).unwrap();
    let key: [u8; 32] = URL_SAFE_NO_PAD
        .decode(&vectors.public_key)
        .unwrap()
        .try_into()
        .unwrap();
    let keys = HashMap::from([(vectors.kid, VerifyingKey::from_bytes(&key).unwrap())]);
    let get_verifier = |reader: Option<&Reader>| match reader {
        None => TicketVerifier {
            keys: keys.clone(),
            issuer: Some(vectors.issuer.clone()),
            audience: Some(vectors.audience.clone()),
        },
        Some(reader) => TicketVerifier {
            keys: if reader.holds_keys {
                keys.clone()
            } else {
                HashMap::new()
            },
            issuer: reader.issuer.clone(),
            audience: reader.audience.clone(),
        },
    };

    let failures: Vec<String> = vectors
        .cases
        .iter()
        .filter_map(|case| {
            let verifier = get_verifier(case.reader.as_ref());
            let state = parse_jwt_ticket(&case.header, &verifier, case.now_in_seconds);
            check_case(case, &state)
        })
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
