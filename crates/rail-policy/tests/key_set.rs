use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::VerifyingKey;
use rail_policy::parse_key_set;
use serde_json::{Value, json};

// The JWT vectors' key, as Rail Center publishes it.
const KID: &str = "vector-key-1";
const X: &str = "A6EHv_POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg";

fn entry() -> Value {
    json!({"kty": "OKP", "crv": "Ed25519", "kid": KID, "x": X, "use": "sig", "alg": "EdDSA"})
}

fn read(set: &Value) -> Option<HashMap<String, VerifyingKey>> {
    parse_key_set(set.to_string().as_bytes())
}

fn kids(set: &Value) -> Option<Vec<String>> {
    let mut kids: Vec<String> = read(set)?.into_keys().collect();
    kids.sort();
    Some(kids)
}

fn with(changes: Value) -> Value {
    let mut entry = entry();
    for (name, value) in changes.as_object().unwrap() {
        entry[name] = value.clone();
    }
    entry
}

#[test]
fn a_published_set_reads_as_its_keys() {
    let x: [u8; 32] = URL_SAFE_NO_PAD.decode(X).unwrap().try_into().unwrap();

    let keys = read(&json!({"keys": [entry()]})).unwrap();

    assert_eq!(
        keys,
        HashMap::from([(KID.to_owned(), VerifyingKey::from_bytes(&x).unwrap())])
    );
}

#[test]
fn an_entry_not_for_verifying_tickets_is_skipped() {
    let mut not_on_curve = [0_u8; 32];
    not_on_curve[0] = 2;
    let skipped = [
        with(json!({"kid": "enc", "use": "enc"})),
        with(json!({"kid": "sign-only", "key_ops": ["sign"]})),
        with(json!({"kid": "rs", "alg": "RS256"})),
        with(json!({"kid": "x448", "crv": "X448"})),
        with(json!({"kid": "short", "x": URL_SAFE_NO_PAD.encode([0_u8; 31])})),
        with(json!({"kid": "off-curve", "x": URL_SAFE_NO_PAD.encode(not_on_curve)})),
        with(json!({"kid": 7})),
        json!({"kty": "RSA", "kid": "rsa", "n": "AQAB", "e": "AQAB"}),
        json!("not an entry"),
    ];
    let mut entries = vec![with(json!({"key_ops": ["verify"]}))];
    entries.extend(skipped.iter().cloned());

    assert_eq!(kids(&json!({"keys": entries})), Some(vec![KID.to_owned()]));
    assert_eq!(kids(&json!({"keys": skipped})), None);
}

#[test]
fn a_kid_shared_by_two_entries_refuses_the_set() {
    let without_kid = || json!({"kty": "RSA", "n": "AQAB", "e": "AQAB"});

    for entries in [
        json!([entry(), entry()]),
        json!([entry(), with(json!({"use": "enc"}))]),
        json!([entry(), without_kid(), without_kid()]),
    ] {
        assert_eq!(kids(&json!({"keys": entries})), None, "{entries}");
    }
}

#[test]
fn what_isnt_an_object_of_keys_is_not_a_key_set() {
    for set in [
        json!({"keys": []}),
        json!({"keys": entry()}),
        json!({}),
        json!([entry()]),
        json!(null),
    ] {
        assert_eq!(kids(&set), None, "{set}");
    }
    assert_eq!(parse_key_set(b"not json"), None);
    assert_eq!(parse_key_set(b"{\"keys\": [\xff]}"), None);
}

#[test]
fn a_leading_byte_order_mark_is_dropped() {
    let body = format!("\u{feff}{}", json!({"keys": [entry()]}));

    assert!(parse_key_set(body.as_bytes()).is_some());
}
