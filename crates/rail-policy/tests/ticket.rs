use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rail_policy::{
    SignedTicketConfig, TicketState, parse_base64_ticket, parse_jwt_ticket, parse_ticket,
};
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
            || Some(ticket.exp_epoch_seconds) != case.exp_in_seconds)
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
            // Every vector's header holds a `.`.
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
fn test_reads_an_empty_header_as_absent() {
    let no_keys = HashMap::new();
    let config = SignedTicketConfig {
        keys: &no_keys,
        issuer: None,
        audience: None,
    };
    assert_eq!(parse_base64_ticket("", 0), TicketState::Absent);
    assert_eq!(parse_jwt_ticket("", &config, 0), TicketState::Absent);
}

fn _sign_ticket(key: &SigningKey, header: &str, payload: &str) -> String {
    let signed = format!("{header}.{payload}");
    let signature = URL_SAFE.encode(key.sign(signed.as_bytes()).to_bytes());
    format!("{signed}.{signature}")
}

#[test]
fn test_reads_padded_parts_in_either_alphabet() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let keys = HashMap::from([("k".to_string(), key.verifying_key())]);
    let config = SignedTicketConfig {
        keys: &keys,
        issuer: Some("i"),
        audience: Some("a"),
    };
    // Padded, and in the standard alphabet: `???` encodes as `Pz8/`.
    let header = URL_SAFE.encode(json!({"alg": "EdDSA", "kid": "k"}).to_string());
    let payload = r#"{"exp": 60, "nbf": 0, "iss": "i", "aud": "a", "x": "???a"}"#;
    let payload = base64::engine::general_purpose::STANDARD.encode(payload);
    assert!(header.ends_with('=') && payload.contains('/') && payload.ends_with('='));

    let ticket = _sign_ticket(&key, &header, &payload);
    let TicketState::Valid(ticket) = parse_jwt_ticket(&ticket, &config, 0) else {
        panic!();
    };
    assert_eq!(ticket.claims["x"], "???a");
}

#[test]
fn test_reads_a_signed_payload_that_is_not_an_object_as_malformed() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let keys = HashMap::from([("k".to_string(), key.verifying_key())]);
    let config = SignedTicketConfig {
        keys: &keys,
        issuer: None,
        audience: None,
    };
    let header = URL_SAFE.encode(json!({"alg": "EdDSA", "kid": "k"}).to_string());
    for payload in ["[]", r#""x""#, "1"] {
        let ticket = _sign_ticket(&key, &header, &URL_SAFE.encode(payload));

        assert!(
            matches!(
                parse_jwt_ticket(&ticket, &config, 0),
                TicketState::Malformed
            ),
            "{payload}"
        );
    }
}

// With the identity point as the key, `[s]B = R + [k]A` holds for `R` the identity and `s` zero,
// whatever the message: `verify` accepts that, `verify_strict` doesn't.
#[test]
fn test_refuses_a_signature_for_a_weak_key() {
    let mut identity = [0; 32];
    identity[0] = 1;
    let key = VerifyingKey::from_bytes(&identity).unwrap();
    let keys = HashMap::from([("k".to_string(), key)]);
    let config = SignedTicketConfig {
        keys: &keys,
        issuer: Some("i"),
        audience: Some("a"),
    };
    let header = URL_SAFE_NO_PAD.encode(json!({"alg": "EdDSA", "kid": "k"}).to_string());
    let payload = json!({"exp": 60, "nbf": 0, "iss": "i", "aud": "a"}).to_string();
    let payload = URL_SAFE_NO_PAD.encode(payload);
    let mut signature = [0; 64];
    signature[0] = 1;
    let signature = URL_SAFE_NO_PAD.encode(signature);

    let ticket = format!("{header}.{payload}.{signature}");
    assert_eq!(
        parse_jwt_ticket(&ticket, &config, 0),
        TicketState::BadSignature
    );
}

#[test]
fn test_reads_an_empty_issuer_or_audience_as_not_configured() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let keys = HashMap::from([("k".to_string(), key.verifying_key())]);
    let header = URL_SAFE_NO_PAD.encode(json!({"alg": "EdDSA", "kid": "k"}).to_string());
    // Each ticket names exactly what the reader holds, so only the empty one refuses it.
    for (issuer, audience) in [("", "a"), ("i", "")] {
        let payload = json!({"exp": 60, "nbf": 0, "iss": issuer, "aud": audience}).to_string();
        let ticket = _sign_ticket(&key, &header, &URL_SAFE_NO_PAD.encode(payload));
        let config = SignedTicketConfig {
            keys: &keys,
            issuer: Some(issuer),
            audience: Some(audience),
        };

        assert_eq!(
            parse_jwt_ticket(&ticket, &config, 0),
            TicketState::WrongAudience,
            "{issuer:?} {audience:?}"
        );
    }
}
