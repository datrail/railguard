use std::collections::{BTreeMap, HashMap};

use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use ed25519_dalek::VerifyingKey;
use rail_policy::{
    Alignment, Baseline, CategoryPosture, OverallPosture, Posture, PostureLevel, TicketClaims,
    TicketState, TicketVerifier, parse_base64_ticket, parse_jwt_ticket,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

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
    /// The expected ticket, where it is valid.
    token: Option<TicketClaims>,
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
        && (Some(&ticket.claims) != case.token.as_ref()
            || Some(ticket.exp_in_seconds) != case.exp_in_seconds)
    {
        return Some(format!("{}: wrong ticket", case.name));
    }
    None
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

fn parse_claims(claims: &Value) -> TicketState {
    parse_base64_ticket(&URL_SAFE.encode(claims.to_string()), 0)
}

#[test]
fn test_reads_every_claim() {
    let claims = json!({
        "id": "10000000-0000-4000-8000-000000000001",
        "agent_id": "20000000-0000-4000-8000-000000000002",
        "sandbox_id": "sandbox-1",
        "sandbox_type": "kubernetes",
        "environment_fingerprint": "fingerprint-1",
        "skills": ["a#/mcp#tools/call#b"],
        "exp": 60,
        "posture": {
            "overall": {"score": 92.5, "level": "medium"},
            "categories": {"identity": {"score": 9, "capped": true}},
        },
        "alignment": {
            "baseline": {"id": "30000000-0000-4000-8000-000000000003", "label": "prod"},
            "drifted": ["llm_model"],
        },
    });
    let TicketState::Valid(ticket) = parse_claims(&claims) else {
        panic!();
    };

    assert_eq!(ticket.exp_in_seconds, 60);
    assert_eq!(
        ticket.claims,
        TicketClaims {
            id: Uuid::from_u128(0x10000000_0000_4000_8000_000000000001),
            agent_id: Uuid::from_u128(0x20000000_0000_4000_8000_000000000002),
            sandbox_id: "sandbox-1".to_string(),
            sandbox_type: Some("kubernetes".to_string()),
            environment_fingerprint: "fingerprint-1".to_string(),
            skills: vec!["a#/mcp#tools/call#b".to_string()],
            posture: Posture {
                overall: OverallPosture {
                    score: 92.5,
                    level: PostureLevel::Medium,
                },
                categories: BTreeMap::from([(
                    "identity".to_string(),
                    CategoryPosture {
                        score: Some(9.0),
                        capped: true,
                    },
                )]),
            },
            alignment: Alignment {
                baseline: Some(Baseline {
                    id: Uuid::from_u128(0x30000000_0000_4000_8000_000000000003),
                    label: "prod".to_string(),
                }),
                drifted: vec!["llm_model".to_string()],
            },
        }
    );
}

#[test]
fn test_reads_the_claims_rail_center_can_leave_out() {
    let claims = json!({
        "id": "10000000-0000-4000-8000-000000000001",
        "agent_id": "20000000-0000-4000-8000-000000000002",
        "sandbox_id": "sandbox-1",
        "sandbox_type": null,
        "environment_fingerprint": "fingerprint-1",
        "skills": [],
        "exp": 60,
        "posture": {
            "overall": {"score": 0.0, "level": "critical"},
            "categories": {"identity": {"score": null, "capped": false}},
        },
        "alignment": {"drifted": []},
    });
    let TicketState::Valid(ticket) = parse_claims(&claims) else {
        panic!();
    };

    assert_eq!(ticket.claims.sandbox_type, None);
    assert_eq!(ticket.claims.alignment.baseline, None);
    assert_eq!(ticket.claims.posture.categories["identity"].score, None);
}
