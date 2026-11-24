use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_model::agent::AgentId;
use rail_runtime::{
    FetchError, OpaqueTicket, RailCenterTickets, Response, TicketSource, Unavailable,
};
use reqwest::StatusCode;

use crate::common::{Reply, client, response, serve};
use crate::holder::{envelope, one, raw, read_at};

// 2026-07-28T10:15:00Z
const EXPIRES_AT: u64 = 1_785_233_700;
const SENT_AT: u64 = EXPIRES_AT - 900;

fn tickets(url: &str) -> RailCenterTickets {
    RailCenterTickets {
        client: client(url),
        agent: AgentId {
            host_id: "host-1".to_owned(),
            sandbox_name: "sandbox a".to_owned(),
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
    // A success too, if not a 200; a 404 and a 401: `ticket_holder_logs`.
    let body = envelope("host-1", &[]);
    let holder = read_at(move || raw(StatusCode::NO_CONTENT, &body), at(SENT_AT)).await;

    assert!(matches!(
        holder.current(),
        Err(Unavailable::IssuerUnreachable)
    ));
}

// Each refused whole, so no ticket is held; another agent's, and why each is refused, are
// `ticket_holder_logs`'.
#[tokio::test(start_paused = true)]
async fn a_ticket_off_the_contract_is_refused() {
    // An empty token, and stamps not of Rail Center's form, beside the logs' rows; a date the
    // calendar refuses is `time`'s to refuse.
    let refused = [
        "not json".to_owned(),
        one("", "2026-07-28T10:15:00Z"),
        one("a.b.c", "2026-07-28T10:15:00+00:00"),
        one("a.b.c", "2026-7-28T10:15:00Z"),
    ];

    for body in refused {
        assert_eq!(
            held(body.clone()).await,
            Err(Unavailable::IssuerUnreachable),
            "{body}"
        );
    }
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
