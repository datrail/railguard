use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_runtime::{
    OpaqueTicket, RailCenterTickets, TicketAnswer, TicketQuery, TicketSettings, TicketSource,
};

use crate::common::{Reply, client, response, serve};
use crate::holder::HOUR;

// 2026-07-28T10:15:00Z
const EXPIRES_AT: u64 = 1_785_233_700;
const SENT_AT: u64 = EXPIRES_AT - 900;

fn query() -> TicketQuery {
    TicketQuery {
        host_id: "host-1".to_owned(),
        sandbox_name: "sandbox a".to_owned(),
    }
}

fn tickets(url: &str) -> RailCenterTickets {
    RailCenterTickets {
        client: client(url),
        settings: TicketSettings {
            query: query(),
            refresh_interval: HOUR,
        },
    }
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn entry(sandbox: &str, token: &str, expires_at: &str) -> String {
    format!(
        r#"{{"agent_id":"6f1c2e2a-6a5e-4b8e-9d3c-0b6c1f8f2a10","sandbox_name":{sandbox},"agent_key":"default","token":"{token}","expires_at":"{expires_at}"}}"#
    )
}

fn envelope(host_id: &str, entries: &[String]) -> String {
    format!(
        r#"{{"host_id":"{host_id}","tickets":[{}]}}"#,
        entries.join(",")
    )
}

fn one(token: &str, expires_at: &str) -> String {
    envelope("host-1", &[entry(r#""sandbox a""#, token, expires_at)])
}

async fn answer(status: &str, body: String) -> Result<TicketAnswer, String> {
    answer_at(status, body, at(SENT_AT)).await
}

async fn answer_at(
    status: &str,
    body: String,
    sent_at: SystemTime,
) -> Result<TicketAnswer, String> {
    let status = status.to_owned();
    let server = serve(move |_| Reply::Raw(response(&status, &[], body.as_bytes()))).await;
    tickets(&server.url).fetch(sent_at).await
}

fn refusal(result: Result<TicketAnswer, String>) -> String {
    result.expect_err("a refusal")
}

#[tokio::test]
async fn the_single_entry_is_read_and_its_expiry_parsed() {
    let answer = answer("200 OK", one("eyJhIjoxfQ==", "2026-07-28T10:15:00Z")).await;

    let Ok(TicketAnswer::Ticket(OpaqueTicket { token, expires_at })) = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(token.expose(), "eyJhIjoxfQ==");
    assert_eq!(expires_at, at(EXPIRES_AT));
}

#[tokio::test]
async fn the_query_names_the_host_and_the_sandbox() {
    let body = one("a.b.c", "2026-07-28T10:15:00Z");
    let server = serve(move |_| Reply::Raw(response("200 OK", &[], body.as_bytes()))).await;

    tickets(&server.url).fetch(at(SENT_AT)).await.unwrap();

    let requests = server.requests.lock().unwrap();
    assert!(
        requests[0].starts_with("GET /v1/tickets?host_id=host-1&sandbox_name=sandbox+a HTTP/1.1"),
        "{}",
        requests[0]
    );
}

#[tokio::test]
async fn an_empty_list_is_an_answer_not_a_failure() {
    let answer = answer("200 OK", envelope("host-1", &[])).await;

    assert_eq!(answer, Ok(TicketAnswer::NoTicket));
}

#[tokio::test]
async fn only_200_is_read() {
    for status in [
        "204 No Content",
        "404 Not Found",
        "409 Conflict",
        "500 Internal Server Error",
    ] {
        let error = refusal(answer(status, envelope("host-1", &[])).await);

        assert_eq!(error, format!("Rail Center answered {status}"));
    }
}

#[tokio::test]
async fn a_ticket_not_naming_this_sandbox_is_refused() {
    for (sandbox, named) in [(r#""sandbox b""#, "sandbox b"), ("null", "none")] {
        let body = envelope("host-1", &[entry(sandbox, "a.b.c", "2026-07-28T10:15:00Z")]);

        let error = refusal(answer("200 OK", body).await);

        assert_eq!(
            error,
            format!("the ticket names sandbox {named}, not sandbox a")
        );
    }
}

#[tokio::test]
async fn more_than_one_ticket_is_refused() {
    let ticket = entry(r#""sandbox a""#, "a.b.c", "2026-07-28T10:15:00Z");

    let error = refusal(answer("200 OK", envelope("host-1", &[ticket.clone(), ticket])).await);

    assert_eq!(
        error,
        "2 tickets for one sandbox; the narrowing wasn't honoured"
    );
}

#[tokio::test]
async fn an_answer_for_another_host_is_refused() {
    let error = refusal(answer("200 OK", envelope("host-2", &[])).await);

    assert_eq!(error, "the response answers for host host-2, not host-1");
}

#[tokio::test]
async fn a_response_off_the_contract_is_refused() {
    let cases = [
        "[]".to_owned(),
        r#"{"host_id":"host-1"}"#.to_owned(),
        r#"{"host_id":"host-1","tickets":{}}"#.to_owned(),
        r#"{"host_id":"host-1","tickets":["a.b.c"]}"#.to_owned(),
        r#"{"host_id":"host-1","tickets":[{"sandbox_name":"sandbox a","expires_at":"2026-07-28T10:15:00Z"}]}"#
            .to_owned(),
        r#"{"host_id":"host-1","tickets":[{"sandbox_name":"sandbox a","token":"a.b.c"}]}"#.to_owned(),
        "not json".to_owned(),
    ];

    for body in cases {
        let error = refusal(answer("200 OK", body.clone()).await);

        assert!(
            error.starts_with("the response is off the contract ("),
            "{body}: {error}"
        );
    }
}

#[tokio::test]
async fn an_expiry_not_in_rail_centers_form_is_refused() {
    for stamp in [
        "2026-07-28",
        "2026-07-28T10:15:00",
        "2026-07-28T10:15:00+00:00",
        "2026-07-28T10:15:00.5Z",
        "2026-07-28t10:15:00z",
        "2026-07-28 10:15:00Z",
        "2026-02-29T10:15:00Z",
        "2026-13-01T10:15:00Z",
        "2026-07-28T24:00:00Z",
        "2026-07-28T10:60:00Z",
        "2026-07-28T10:15:60Z",
        "2100-02-29T10:15:00Z",
        "+026-07-28T10:15:00Z",
    ] {
        let error = refusal(answer("200 OK", one("a.b.c", stamp)).await);

        assert_eq!(
            error,
            format!("expires_at isn't YYYY-MM-DDTHH:MM:SSZ: {stamp}"),
            "{stamp}"
        );
    }
}

#[tokio::test]
async fn a_leap_day_is_read() {
    let answer = answer_at(
        "200 OK",
        one("a.b.c", "2028-02-29T00:00:00Z"),
        at(1_835_395_200 - 60),
    )
    .await;

    let Ok(TicketAnswer::Ticket(ticket)) = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(ticket.expires_at, at(1_835_395_200));
}

#[tokio::test]
async fn an_expiry_in_the_past_is_a_ticket_not_an_error() {
    let answer = answer_at(
        "200 OK",
        one("a.b.c", "2026-07-28T10:15:00Z"),
        at(EXPIRES_AT + 60),
    )
    .await;

    let Ok(TicketAnswer::Ticket(ticket)) = answer else {
        panic!("{answer:?}");
    };
    assert!(!ticket.is_valid(at(EXPIRES_AT + 60)));
}

#[tokio::test]
async fn a_ticket_stops_being_valid_at_its_expiry() {
    let Ok(TicketAnswer::Ticket(ticket)) =
        answer("200 OK", one("a.b.c", "2026-07-28T10:15:00Z")).await
    else {
        panic!();
    };

    assert!(ticket.is_valid(at(EXPIRES_AT - 1)));
    assert!(!ticket.is_valid(at(EXPIRES_AT)));
}

#[tokio::test]
async fn a_lifetime_past_what_rail_center_stamps_is_refused() {
    let day_and_margin = 86_400 + 300;
    let held = answer_at(
        "200 OK",
        one("a.b.c", "2026-07-28T10:15:00Z"),
        at(EXPIRES_AT - day_and_margin),
    )
    .await;
    let refused = answer_at(
        "200 OK",
        one("a.b.c", "2026-07-28T10:15:00Z"),
        at(EXPIRES_AT - day_and_margin - 1),
    )
    .await;

    assert!(matches!(held, Ok(TicketAnswer::Ticket(_))), "{held:?}");
    assert_eq!(
        refused.unwrap_err(),
        "the ticket expires more than 86700 s away, past what Rail Center stamps"
    );
}

#[tokio::test]
async fn a_token_rail_center_cannot_mint_is_refused_without_being_quoted() {
    for token in ["", "a b", "a+b/c", "tok\\u00e9n", "a\\nb"] {
        let error = refusal(answer("200 OK", one(token, "2026-07-28T10:15:00Z")).await);

        assert!(
            error.starts_with("the ticket isn't base64url ("),
            "{token}: {error}"
        );
        assert!(token.is_empty() || !error.contains(token), "{error}");
    }
}

#[tokio::test]
async fn a_body_past_the_cap_is_refused() {
    let padding = "x".repeat(256 * 1024);
    let body = format!(r#"{{"host_id":"host-1","tickets":[],"padding":"{padding}"}}"#);

    let error = refusal(answer("200 OK", body).await);

    assert_eq!(error, "the response is past the 262144 bytes read");
}

#[tokio::test]
async fn a_response_off_the_contract_is_described_without_its_values() {
    let body = r#"{"host_id":"host-1","tickets":"eyJ0aGUiOiJ0aWNrZXQifQ"}"#.to_owned();

    let error = refusal(answer("200 OK", body).await);

    assert!(
        error.starts_with("the response is off the contract ("),
        "{error}"
    );
    assert!(!error.contains("eyJ0aGUiOiJ0aWNrZXQifQ"), "{error}");
}

// On paused time the deadline fires at once: the error names it, and when.
#[tokio::test(start_paused = true)]
async fn a_silent_rail_center_is_given_up_on_after_10_s() {
    let server = serve(|_| Reply::Silent).await;

    let error = refusal(tickets(&server.url).fetch(at(SENT_AT)).await);

    assert_eq!(error, "the fetch ran past its 10s deadline");
}

#[tokio::test(start_paused = true)]
async fn a_dribbling_answer_is_given_up_on_after_10_s() {
    let server =
        serve(|_| Reply::Dribble(b"HTTP/1.1 200 OK\r\n\r\n".to_vec(), Duration::from_secs(1)))
            .await;

    let error = refusal(tickets(&server.url).fetch(at(SENT_AT)).await);

    assert_eq!(error, "the fetch ran past its 10s deadline");
}
