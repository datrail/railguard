use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rail_model::policy::{SignedTicketConfig, TicketState, parse_base64_ticket, parse_jwt_ticket};
use serde_json::json;

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
