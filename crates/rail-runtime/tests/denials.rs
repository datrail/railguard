use rail_policy::{
    ChainOutcome, Decision, Endpoint, FallbackDenial, Outcome, Refusal, TicketState,
};
use rail_runtime::{Denial, DenialSink, DeniedBy, RailCenterDenials};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{Reply, TOKEN, client, response, serve};
use crate::sink::{POLICY, Sink, resolved, valid};

const TICKET_ID: &str = "0d9b6f3c-8a52-4e1f-9c7a-2b3e4f5a6b7c";

// The report sent for `denial`.
async fn report_of(denial: Denial<'_>) -> Value {
    let sink = Sink::accepting();
    sink.start().report(&denial);
    sink.until_sent(1).await;
    let [report] = &sink.reports()[..] else {
        panic!("{:?}", sink.reports())
    };
    report.clone()
}

fn denial<'a>(denied_by: DeniedBy, endpoint: &'a Endpoint, ticket: &'a TicketState) -> Denial<'a> {
    Denial {
        denied_by,
        endpoint,
        ticket,
        claimed_status: None,
    }
}

#[tokio::test]
async fn a_policy_denial_names_the_policy_the_key_without_its_slug_and_the_ticket() {
    let ticket = valid(json!({"id": TICKET_ID, "agent_id": "a", "posture_score": 7}));
    let endpoint = resolved();

    let report = report_of(denial(DeniedBy::Policy(POLICY), &endpoint, &ticket)).await;

    assert_eq!(
        report,
        json!({
            "policy_id": "11111111-0000-4000-8000-000000000001",
            "endpoint_key": "/mcp#tools/call#getPrice",
            "denied_at": "2026-11-06T12:00:00.512247Z",
            "ticket_id": TICKET_ID,
            "metadata": {"endpoint_resolution": "resolved", "x-rail-status": "valid"},
        })
    );
}

#[tokio::test]
async fn a_fallback_denial_names_the_fallback_and_no_policy() {
    for (fallback, name) in [
        (FallbackDenial::Ticket, "ticket"),
        (FallbackDenial::Binding, "binding"),
    ] {
        let endpoint = resolved();

        let report = report_of(denial(
            DeniedBy::Fallback(fallback),
            &endpoint,
            &TicketState::Absent,
        ))
        .await;

        assert_eq!(
            report,
            json!({
                "endpoint_key": "/mcp#tools/call#getPrice",
                "denied_at": "2026-11-06T12:00:00.512247Z",
                "metadata": {
                    "endpoint_resolution": "resolved",
                    "x-rail-status": "absent",
                    "fallback": name,
                },
            }),
            "{name}"
        );
    }
}

#[tokio::test]
async fn a_call_naming_no_endpoint_reports_a_null_key_and_why() {
    for (endpoint, resolution) in [
        (Endpoint::Keyless, "keyless"),
        (Endpoint::Unrecognised, "unrecognised"),
    ] {
        let report = report_of(denial(
            DeniedBy::Policy(POLICY),
            &endpoint,
            &TicketState::Absent,
        ))
        .await;

        assert_eq!(report["endpoint_key"], Value::Null, "{resolution}");
        assert_eq!(
            report["metadata"]["endpoint_resolution"], resolution,
            "{resolution}"
        );
    }
}

#[tokio::test]
async fn each_ticket_state_is_reported_as_this_reader_read_it() {
    let states = [
        (TicketState::Absent, "absent"),
        (TicketState::Undecodable, "undecodable"),
        (TicketState::BadSignature, "bad_signature"),
        (TicketState::Malformed, "malformed"),
        (TicketState::WrongAudience, "wrong_audience"),
        (TicketState::Expired, "expired"),
        (valid(json!({})), "valid"),
    ];
    for (ticket, state) in states {
        let endpoint = resolved();

        let report = report_of(denial(DeniedBy::Policy(POLICY), &endpoint, &ticket)).await;

        assert_eq!(report["metadata"]["x-rail-status"], state);
        assert!(report.get("ticket_id").is_none(), "{state}: {report}");
    }
}

#[tokio::test]
async fn a_ticket_id_is_sent_canonical_and_only_when_it_reads_as_a_uuid() {
    let ids = [
        (json!(TICKET_ID.to_uppercase()), Some(TICKET_ID)),
        (json!(TICKET_ID.replace('-', "")), Some(TICKET_ID)),
        (json!(format!("{{{TICKET_ID}}}")), Some(TICKET_ID)),
        (json!(format!("urn:uuid:{TICKET_ID}")), Some(TICKET_ID)),
        (json!("not-a-uuid"), None),
        (json!(format!("{TICKET_ID}0")), None),
        (json!(42), None),
        (Value::Null, None),
    ];
    for (id, sent) in ids {
        let ticket = valid(json!({"id": id}));
        let endpoint = resolved();

        let report = report_of(denial(DeniedBy::Policy(POLICY), &endpoint, &ticket)).await;

        assert_eq!(
            report.get("ticket_id").and_then(Value::as_str),
            sent,
            "{id}"
        );
    }
}

#[tokio::test]
async fn a_claimed_status_is_reported_apart_cut_at_255_characters() {
    let long = "é".repeat(300);
    for (claimed, reported) in [
        ("not-found", "not-found".to_owned()),
        (long.as_str(), "é".repeat(255)),
    ] {
        let endpoint = resolved();
        let report = report_of(Denial {
            claimed_status: Some(claimed),
            ..denial(DeniedBy::Policy(POLICY), &endpoint, &TicketState::Absent)
        })
        .await;

        assert_eq!(report["metadata"]["claimed-x-rail-status"], reported);
        assert_eq!(report["metadata"]["x-rail-status"], "absent");
    }
}

fn chain(decision: Decision) -> Option<ChainOutcome> {
    Some(ChainOutcome {
        decision,
        alerts: Vec::new(),
    })
}

#[test]
fn what_denied_is_the_first_fallback_else_the_policy_and_never_a_refusal() {
    let outcomes = [
        (
            Outcome {
                fallback_denials: vec![FallbackDenial::Ticket, FallbackDenial::Binding],
                chain: chain(Decision::Denied(POLICY)),
            },
            Some(DeniedBy::Fallback(FallbackDenial::Ticket)),
        ),
        (
            Outcome {
                fallback_denials: vec![FallbackDenial::Binding],
                chain: None,
            },
            Some(DeniedBy::Fallback(FallbackDenial::Binding)),
        ),
        (
            Outcome {
                fallback_denials: Vec::new(),
                chain: chain(Decision::Denied(POLICY)),
            },
            Some(DeniedBy::Policy(POLICY)),
        ),
        (
            Outcome {
                fallback_denials: Vec::new(),
                chain: chain(Decision::Allowed),
            },
            None,
        ),
        (
            Outcome {
                fallback_denials: Vec::new(),
                chain: chain(Decision::Refused {
                    policy: POLICY,
                    refusal: Refusal::BudgetExceeded,
                }),
            },
            None,
        ),
    ];
    for (outcome, denied_by) in outcomes {
        assert_eq!(
            format!("{:?}", DeniedBy::of(&outcome)),
            format!("{denied_by:?}"),
            "{outcome:?}"
        );
    }
}

#[tokio::test]
async fn past_64_in_flight_a_report_is_dropped_and_counted_without_waiting() {
    let sink = Sink::holding();
    let reporter = sink.start();
    let endpoint = resolved();
    let policy = denial(DeniedBy::Policy(POLICY), &endpoint, &TicketState::Absent);

    for _ in 0..65 {
        reporter.report(&policy);
    }
    sink.until_sent(64).await;
    tokio::task::yield_now().await;

    assert_eq!(sink.reports().len(), 64);
    assert_eq!(reporter.dropped(), 1);

    // Each answered report frees its place.
    sink.release.add_permits(1);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    reporter.report(&policy);
    sink.until_sent(65).await;
    assert_eq!(reporter.dropped(), 1);
}

#[tokio::test]
async fn reports_naming_no_policy_hold_16_places_at_most() {
    let sink = Sink::holding();
    let reporter = sink.start();
    let endpoint = resolved();
    let fallback = denial(
        DeniedBy::Fallback(FallbackDenial::Binding),
        &endpoint,
        &TicketState::Absent,
    );
    let policy = denial(DeniedBy::Policy(POLICY), &endpoint, &TicketState::Absent);

    for _ in 0..20 {
        reporter.report(&fallback);
    }
    for _ in 0..48 {
        reporter.report(&policy);
    }
    sink.until_sent(64).await;
    tokio::task::yield_now().await;

    let naming_no_policy = sink
        .reports()
        .iter()
        .filter(|report| report.get("policy_id").is_none())
        .count();
    assert_eq!((sink.reports().len(), naming_no_policy), (64, 16));
    assert_eq!(reporter.dropped(), 4);

    // With all 64 places taken, one naming a policy is dropped too.
    reporter.report(&policy);
    assert_eq!(reporter.dropped(), 5);
}

#[tokio::test]
async fn an_answered_report_naming_no_policy_frees_its_place() {
    let sink = Sink::holding();
    let reporter = sink.start();
    let endpoint = resolved();
    let fallback = denial(
        DeniedBy::Fallback(FallbackDenial::Binding),
        &endpoint,
        &TicketState::Absent,
    );

    for _ in 0..16 {
        reporter.report(&fallback);
    }
    sink.until_sent(16).await;
    // Still in flight, so still holding their places.
    reporter.report(&fallback);
    assert_eq!(reporter.dropped(), 1);

    sink.release.add_permits(16);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    for _ in 0..16 {
        reporter.report(&fallback);
    }
    sink.until_sent(32).await;

    assert_eq!(reporter.dropped(), 1);
}

#[tokio::test]
async fn a_report_naming_no_policy_dropped_at_the_64_cap_holds_no_place() {
    let sink = Sink::holding();
    let reporter = sink.start();
    let endpoint = resolved();
    let fallback = denial(
        DeniedBy::Fallback(FallbackDenial::Binding),
        &endpoint,
        &TicketState::Absent,
    );
    let policy = denial(DeniedBy::Policy(POLICY), &endpoint, &TicketState::Absent);

    for _ in 0..64 {
        reporter.report(&policy);
    }
    reporter.report(&fallback);
    assert_eq!(reporter.dropped(), 1);

    sink.until_sent(64).await;
    sink.release.add_permits(64);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    for _ in 0..16 {
        reporter.report(&fallback);
    }
    sink.until_sent(80).await;

    assert_eq!(reporter.dropped(), 1);
}

#[tokio::test]
async fn a_report_is_posted_once_to_rail_center_with_the_credential() {
    type Replying = fn() -> Reply;
    let replies: [(&str, Replying); 3] = [
        ("accepted", || {
            Reply::Raw(response("202 Accepted", &[], b"{}"))
        }),
        ("refused", || {
            Reply::Raw(response("422 Unprocessable", &[], b""))
        }),
        ("dropped", || Reply::Close),
    ];
    for (label, reply) in replies {
        let server = serve(move |_| reply()).await;
        let sink = RailCenterDenials {
            client: client(&server.url),
        };
        let answer = sink.send(br#"{"denied_at": "x"}"#.to_vec()).await;

        let requests = server.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1, "{label}: {requests:?}");
        let request = requests[0].to_ascii_lowercase();
        assert!(
            request.starts_with("post /v1/denials "),
            "{label}: {request}"
        );
        assert!(
            request.contains("content-type: application/json\r\n"),
            "{label}"
        );
        assert!(
            request.contains(&format!(
                "authorization: bearer {}\r\n",
                TOKEN.to_ascii_lowercase()
            )),
            "{label}"
        );
        assert!(
            request.ends_with(r#"{"denied_at": "x"}"#),
            "{label}: {request}"
        );
        match label {
            "accepted" => assert_eq!(answer.unwrap().status, StatusCode::ACCEPTED),
            "refused" => assert_eq!(answer.unwrap().status, StatusCode::UNPROCESSABLE_ENTITY),
            _ => assert!(answer.is_err()),
        }
    }
}
