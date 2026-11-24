use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_model::agent::AgentId;
use rail_runtime::{
    FetchError, OpaqueTicket, RailCenterTickets, Response, TicketSettings, TicketSource,
    Unavailable,
};
use reqwest::StatusCode;

use crate::common::{Reply, client, response, serve};
use crate::holder::{HOUR, envelope, one, raw, read_at};

// 2026-07-28T10:15:00Z
const EXPIRES_AT: u64 = 1_785_233_700;
const SENT_AT: u64 = EXPIRES_AT - 900;

fn tickets(url: &str) -> RailCenterTickets {
    RailCenterTickets {
        client: client(url),
        settings: TicketSettings {
            agent: AgentId {
                host_id: "host-1".to_owned(),
                sandbox_name: "sandbox a".to_owned(),
            },
            refresh_interval: HOUR,
        },
    }
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

// What a 200 carrying `body`, fetched at `sent_at`, leaves held.
async fn held_at(body: String, sent_at: SystemTime) -> Result<OpaqueTicket, Unavailable> {
    let holder = read_at(move || raw(StatusCode::OK, &body), sent_at).await;
    holder.current().map(|ticket| (*ticket).clone())
}

async fn held(body: String) -> Result<OpaqueTicket, Unavailable> {
    held_at(body, at(SENT_AT)).await
}

fn refusal(result: Result<Response, FetchError>) -> String {
    result.expect_err("a refusal").to_string()
}

#[tokio::test(start_paused = true)]
async fn the_single_entry_is_read_and_its_expiry_parsed() {
    let ticket = held(one("eyJhIjoxfQ==", "2026-07-28T10:15:00Z"))
        .await
        .unwrap();

    assert_eq!(ticket.token.expose(), "eyJhIjoxfQ==");
    assert_eq!(ticket.expires_at, at(EXPIRES_AT));
}

#[tokio::test(start_paused = true)]
async fn only_200_is_read() {
    // A 404 and a 401 too: `ticket_holder_logs`.
    for status in [StatusCode::NO_CONTENT, StatusCode::INTERNAL_SERVER_ERROR] {
        let body = envelope("host-1", &[]);
        let holder = read_at(move || raw(status, &body), at(SENT_AT)).await;

        assert!(
            matches!(holder.current(), Err(Unavailable::IssuerUnreachable)),
            "{status}"
        );
    }
}

// Each refused whole, so no ticket is held; another agent's, and why each is refused, are
// `ticket_holder_logs`'.
#[tokio::test(start_paused = true)]
async fn a_ticket_off_the_contract_is_refused() {
    let mut refused = vec![
        "[]".to_owned(),
        r#"{"host_id":"host-1"}"#.to_owned(),
        r#"{"host_id":"host-1","tickets":{}}"#.to_owned(),
        r#"{"host_id":"host-1","tickets":["a.b.c"]}"#.to_owned(),
        r#"{"host_id":"host-1","tickets":[{"sandbox_name":"sandbox a","expires_at":"2026-07-28T10:15:00Z"}]}"#
            .to_owned(),
        r#"{"host_id":"host-1","tickets":[{"sandbox_name":"sandbox a","token":"a.b.c"}]}"#.to_owned(),
        "not json".to_owned(),
    ];
    for token in ["", "a b", "a+b/c", "tok\\u00e9n", "a\\nb"] {
        refused.push(one(token, "2026-07-28T10:15:00Z"));
    }
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
        refused.push(one("a.b.c", stamp));
    }

    for body in refused {
        assert_eq!(
            held(body.clone()).await,
            Err(Unavailable::IssuerUnreachable),
            "{body}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_leap_day_is_read() {
    let ticket = held_at(one("a.b.c", "2028-02-29T00:00:00Z"), at(1_835_395_200 - 60))
        .await
        .unwrap();

    assert_eq!(ticket.expires_at, at(1_835_395_200));
}

#[tokio::test(start_paused = true)]
async fn a_lifetime_past_what_rail_center_stamps_is_refused() {
    let day_and_margin = 86_400 + 300;
    let body = || one("a.b.c", "2026-07-28T10:15:00Z");

    let kept = held_at(body(), at(EXPIRES_AT - day_and_margin)).await;
    let refused = held_at(body(), at(EXPIRES_AT - day_and_margin - 1)).await;

    assert!(kept.is_ok(), "{kept:?}");
    assert_eq!(refused, Err(Unavailable::IssuerUnreachable));
}

#[tokio::test]
async fn the_query_names_the_host_and_the_sandbox() {
    let body = envelope("host-1", &[]);
    let server = serve(move |_| Reply::Raw(response("200 OK", &[], body.as_bytes()))).await;

    tickets(&server.url).fetch().await.unwrap();

    let requests = server.requests.lock().unwrap();
    assert!(
        requests[0].starts_with("GET /v1/tickets?host_id=host-1&sandbox_name=sandbox+a HTTP/1.1"),
        "{}",
        requests[0]
    );
}

#[tokio::test]
async fn a_body_past_the_cap_is_refused() {
    let padding = "x".repeat(256 * 1024);
    let body = format!(r#"{{"host_id":"host-1","tickets":[],"padding":"{padding}"}}"#);
    let server = serve(move |_| Reply::Raw(response("200 OK", &[], body.as_bytes()))).await;

    let error = refusal(tickets(&server.url).fetch().await);

    assert_eq!(error, "the response is past the 262144 bytes read");
}

// On paused time the first timer due fires at once: the error names which, and when.
#[tokio::test(start_paused = true)]
async fn a_silent_rail_center_is_given_up_on_after_5_s() {
    let server = serve(|_| Reply::Silent).await;

    let error = refusal(tickets(&server.url).fetch().await);

    assert_eq!(error, "Rail Center was silent for 5s");
}

#[tokio::test(start_paused = true)]
async fn a_dribbling_answer_is_given_up_on_after_10_s() {
    let server =
        serve(|_| Reply::Dribble(b"HTTP/1.1 200 OK\r\n\r\n".to_vec(), Duration::from_secs(1)))
            .await;

    let error = refusal(tickets(&server.url).fetch().await);

    assert_eq!(error, "the fetch ran past its 10s deadline");
}
