use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::VerifyingKey;
use rail_policy::{DroppedKey, parse_key_set};
use serde_json::{Value, json};

// The JWT vectors' key, as Rail Center publishes it.
const KID: &str = "vector-key-1";
const X: &str = "A6EHv_POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg";

fn entry() -> Value {
    json!({"kty": "OKP", "crv": "Ed25519", "kid": KID, "x": X, "use": "sig", "alg": "EdDSA"})
}

fn read(set: &Value) -> Option<HashMap<String, VerifyingKey>> {
    parse_key_set(set.to_string().as_bytes()).map(|read| read.keys)
}

fn dropped(set: &Value) -> Vec<(Option<String>, &'static str)> {
    let read = parse_key_set(set.to_string().as_bytes()).unwrap();
    read.dropped
        .into_iter()
        .map(|DroppedKey { kid, reason }| (kid, reason))
        .collect()
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
fn an_entry_for_another_purpose_is_skipped_without_a_word() {
    let other = [
        with(json!({"kid": "enc", "use": "enc"})),
        with(json!({"kid": "sign-only", "key_ops": ["sign"]})),
        with(json!({"kid": "rs", "alg": "RS256"})),
        with(json!({"kid": "x448", "crv": "X448"})),
        json!({"kty": "RSA", "kid": "rsa", "n": "AQAB", "e": "AQAB"}),
        json!("not an entry"),
    ];
    let mut entries = vec![with(json!({"key_ops": ["verify"]}))];
    entries.extend(other.iter().cloned());
    let set = json!({ "keys": entries });

    assert_eq!(kids(&set), Some(vec![KID.to_owned()]));
    assert_eq!(dropped(&set), []);
    assert_eq!(kids(&json!({ "keys": other })), Some(vec![]));
}

#[test]
fn a_signing_key_that_cant_be_used_is_dropped_saying_why() {
    let mut not_on_curve = [0_u8; 32];
    not_on_curve[0] = 2;
    let set = json!({"keys": [
        entry(),
        with(json!({"kid": 7})),
        with(json!({"kid": "no-x", "x": null})),
        with(json!({"kid": "not-base64", "x": "!!"})),
        with(json!({"kid": "short", "x": URL_SAFE_NO_PAD.encode([0_u8; 31])})),
        with(json!({"kid": "off-curve", "x": URL_SAFE_NO_PAD.encode(not_on_curve)})),
    ]});

    assert_eq!(kids(&set), Some(vec![KID.to_owned()]));
    let some = |kid: &str| Some(kid.to_owned());
    assert_eq!(
        dropped(&set),
        [
            (some("7"), "its kid isn't a string"),
            (some("no-x"), "its x isn't a string"),
            (some("not-base64"), "its x isn't base64"),
            (some("short"), "its x isn't 32 bytes"),
            (
                some("off-curve"),
                "its x isn't a point on the Ed25519 curve"
            ),
        ]
    );
    let mut without_kid = entry();
    without_kid.as_object_mut().unwrap().remove("kid");
    assert_eq!(
        dropped(&json!({ "keys": [without_kid] })),
        [(None, "its kid isn't a string")]
    );
}

#[test]
fn a_kid_shared_by_two_entries_refuses_the_set() {
    let without_kid = || json!({"kty": "RSA", "n": "AQAB", "e": "AQAB"});

    for entries in [
        json!([entry(), entry()]),
        json!([entry(), with(json!({"use": "enc"}))]),
        json!([entry(), without_kid(), without_kid()]),
        json!([entry(), without_kid(), {"kty": "RSA", "kid": null}]),
    ] {
        assert_eq!(kids(&json!({"keys": entries})), None, "{entries}");
    }
}

#[test]
fn what_isnt_an_object_of_keys_is_not_a_key_set() {
    for set in [
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
