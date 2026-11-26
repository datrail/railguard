use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::VerifyingKey;
use rail_model::policy::{
    SignedTicketConfig, TicketState, parse_base64_ticket, parse_jwt_ticket, parse_ticket,
};
use serde::Deserialize;
use serde_json::{Value, json};

const BASE64_VECTORS: &str = include_str!("../../../../vectors/policy/base64_ticket.json");
const JWT_VECTORS: &str = include_str!("../../../../vectors/policy/jwt_ticket.json");

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
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
        return Some(format!("{}: want {}, got {name}", case.id, case.state));
    }
    if let TicketState::Valid(ticket) = state
        && (Some(_read_numbers_as_doubles(Value::Object(
            ticket.claims.clone(),
        ))) != case.token.clone().map(_read_numbers_as_doubles)
            || Some(ticket.exp_epoch_seconds) != case.exp_in_seconds)
    {
        return Some(format!("{}: wrong ticket", case.id));
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
    assert!(!vectors.cases.is_empty());

    let no_keys = HashMap::new();
    let config = SignedTicketConfig {
        keys: &no_keys,
        issuer: None,
        audience: None,
    };
    let failures: Vec<String> = vectors
        .cases
        .iter()
        .filter_map(|case| {
            let state = parse_base64_ticket(&case.header, case.now_in_seconds);
            // No vector's header holds a `.`.
            assert_eq!(
                parse_ticket(&case.header, &config, case.now_in_seconds),
                state
            );
            check_case(case, &state)
        })
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn test_jwt_vectors() {
    let vectors: JwtVectors = serde_json::from_str(JWT_VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());
    let key: [u8; 32] = URL_SAFE_NO_PAD
        .decode(&vectors.public_key)
        .unwrap()
        .try_into()
        .unwrap();
    let keys = HashMap::from([(vectors.kid, VerifyingKey::from_bytes(&key).unwrap())]);
    let no_keys = HashMap::new();
    let failures: Vec<String> = vectors
        .cases
        .iter()
        .filter_map(|case| {
            let config = match &case.reader {
                None => SignedTicketConfig {
                    keys: &keys,
                    issuer: Some(&vectors.issuer),
                    audience: Some(&vectors.audience),
                },
                Some(reader) => SignedTicketConfig {
                    keys: if reader.holds_keys { &keys } else { &no_keys },
                    issuer: reader.issuer.as_deref(),
                    audience: reader.audience.as_deref(),
                },
            };
            let state = parse_jwt_ticket(&case.header, &config, case.now_in_seconds);
            // Every vector's header but the empty one holds a `.`.
            assert_eq!(
                parse_ticket(&case.header, &config, case.now_in_seconds),
                state
            );
            check_case(case, &state)
        })
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
