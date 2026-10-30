use rail_policy::{TicketState, parse_base64_ticket};
use serde::Deserialize;
use serde_json::Value;

const VECTORS: &str = include_str!("../../../vectors/parse_base64_ticket_conformance_cases.json");

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
    /// The expected claims of a valid ticket.
    token: Option<Value>,
}

#[test]
fn test_vectors() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();

    let mut failures = Vec::new();
    for case in vectors.cases {
        let state = parse_base64_ticket(&case.header, case.now_in_seconds);
        let name = match &state {
            TicketState::Absent => "absent",
            TicketState::Undecodable => "undecodable",
            TicketState::BadSignature => "bad_signature",
            TicketState::Malformed => "malformed",
            TicketState::WrongAudience => "wrong_audience",
            TicketState::Expired => "expired",
            TicketState::Valid(ticket) => {
                if Some(&ticket.claims) != case.token.as_ref()
                    || Some(ticket.exp_in_seconds) != ticket.claims["exp"].as_i64()
                {
                    failures.push(format!("{}: wrong ticket", case.name));
                }
                "valid"
            }
        };
        if name != case.state {
            failures.push(format!("{}: want {}, got {name}", case.name, case.state));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
