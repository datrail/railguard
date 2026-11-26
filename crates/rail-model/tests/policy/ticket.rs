use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::VerifyingKey;
use rail_model::policy::{SignedTicketConfig, TicketState, parse_jwt_ticket};
use serde_json::json;

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
