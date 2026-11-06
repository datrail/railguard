use std::sync::Mutex;
use std::time::Duration;

use rail_runtime::{BundleHolder, BundleSource, FetchError, Jitter, RailCenterBundles};
use reqwest::StatusCode;
use serde_json::json;
use tokio::runtime::Handle;
use tokio::time::Instant;

use crate::bundles::{Answer, MINUTE, Script, answer, bundle, failed, held_hash, ok, start};
use crate::common::{self, Reply, client, response, serve};
use crate::holder::{Clock, until};

const LIMIT: Duration = Duration::from_secs(2);

// Every way of getting no usable bundle, a bundle Rail Center sent or not.
fn unusable_answers() -> Vec<(&'static str, Answer)> {
    vec![
        ("a 503", answer(StatusCode::SERVICE_UNAVAILABLE, b"")),
        ("a 500", answer(StatusCode::INTERNAL_SERVER_ERROR, b"")),
        ("a 404", answer(StatusCode::NOT_FOUND, b"")),
        (
            "a refused credential",
            answer(StatusCode::UNAUTHORIZED, b""),
        ),
        ("a forbidden read", answer(StatusCode::FORBIDDEN, b"")),
        (
            "a connection failure",
            Err(FetchError::Transport("connection refused".to_owned())),
        ),
        ("a deadline", failed()),
        (
            "a body that isn't JSON",
            answer(StatusCode::OK, b"<html>nope</html>"),
        ),
        ("an empty body", answer(StatusCode::OK, b"")),
        (
            "a body that isn't UTF-8",
            answer(StatusCode::OK, b"{\"\xff\":1}"),
        ),
        (
            "a 202 carrying a good bundle",
            answer(
                StatusCode::ACCEPTED,
                bundle("v2", json!({})).to_string().as_bytes(),
            ),
        ),
        ("a JSON array", ok(&json!([]))),
        (
            "another major",
            ok(&bundle("v2", json!({"schema_version": "3.0"}))),
        ),
        (
            "a version not MAJOR.MINOR",
            ok(&bundle("v2", json!({"schema_version": "2.0.1"}))),
        ),
        (
            "policies not a list",
            ok(&bundle("v2", json!({"policies": "not a list"}))),
        ),
        ("no posture", {
            let mut body = bundle("v2", json!({}));
            body.as_object_mut().unwrap().remove("enforcement");
            ok(&body)
        }),
        (
            "a posture outside the contract",
            ok(&bundle("v2", json!({"enforcement": {"mode": "audit"}}))),
        ),
        ("no content_hash", {
            let mut body = bundle("v2", json!({}));
            body.as_object_mut().unwrap().remove("content_hash");
            ok(&body)
        }),
    ]
}

#[tokio::test(start_paused = true)]
async fn only_a_usable_answer_is_held_and_any_other_is_retried_as_a_failure() {
    for ((label, first), (_, later)) in unusable_answers().into_iter().zip(unusable_answers()) {
        let clock = Clock::new();
        let answers = Mutex::new(vec![first, ok(&bundle("v1", json!({}))), later].into_iter());
        let script = Script::new(&clock, move |_| {
            answers.lock().unwrap().next().unwrap_or_else(failed)
        });

        let holder = start(&script, MINUTE);
        until(&clock, 0).await;
        assert_eq!(held_hash(&holder), None, "{label}");
        until(&clock, 65).await;

        assert_eq!(script.fetched_at(), [0, 5, 65], "{label}");
        assert_eq!(held_hash(&holder).as_deref(), Some("v1"), "{label}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_long_outage_keeps_backing_off_at_the_interval() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| failed());

    let _holder = start(&script, MINUTE);
    until(&clock, 35 + 40 * 60).await;

    // 0, 5, 15, 35 and 75, then a minute apart: 44 failures, past where 2^n overflows a u32.
    let fetched_at = script.fetched_at();
    assert_eq!(fetched_at.len(), 44);
    assert!(
        fetched_at
            .windows(2)
            .skip(4)
            .all(|pair| pair[1] - pair[0] == 60),
        "{fetched_at:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_wait_ends_once_no_attempt_can_land_within_its_limit() {
    // The retry is due at 5: asked at 3 it can't land in time, asked at 4 it fails, the next at 15.
    for (asked_at, waited, fetched_at) in [
        (3, Duration::ZERO, [0].as_slice()),
        (4, Duration::from_secs(1), &[0, 5]),
    ] {
        let clock = Clock::new();
        let script = Script::new(&clock, |_| failed());

        let holder = start(&script, MINUTE);
        until(&clock, asked_at).await;
        let started = Instant::now();
        let held = holder.wait_for_bundle(LIMIT).await;

        assert!(held.is_none(), "{asked_at}");
        assert_eq!(started.elapsed(), waited, "{asked_at}");
        assert_eq!(script.fetched_at(), fetched_at, "{asked_at}");
    }
}

// Real time: the fetch goes over a socket, which paused time would race.
#[tokio::test]
async fn the_route_the_gateway_and_the_credential_travel_with_the_request() {
    let body = bundle("v1", json!({})).to_string();
    let server = serve(move |_| Reply::Raw(response("200 OK", &[], body.as_bytes()))).await;
    let source = RailCenterBundles {
        client: client(&format!("{}/", server.url)),
        gateway_slug: "edge".to_owned(),
    };

    let holder = BundleHolder::start(&Handle::current(), source, MINUTE, Jitter::new(|| 1.0));
    let held = holder.wait_for_bundle(Duration::from_secs(10)).await;

    assert_eq!(held.unwrap().content_hash, "v1");
    let requests = server.requests.lock().unwrap();
    let request = requests[0].to_ascii_lowercase();
    assert!(
        request.starts_with("get /v1/policy-bundle?gateway=edge http/1.1\r\n"),
        "{request}"
    );
    assert!(request.contains(&format!(
        "authorization: bearer {}",
        common::TOKEN.to_ascii_lowercase()
    )));
    assert!(
        request.contains("accept: application/json\r\n"),
        "{request}"
    );
    assert!(
        request.contains("accept-encoding: gzip, identity\r\n"),
        "{request}"
    );
}

fn bundles(url: &str) -> RailCenterBundles {
    RailCenterBundles {
        client: client(url),
        gateway_slug: "edge".to_owned(),
    }
}

// On paused time the first timer due fires at once: the error names which, and when.
#[tokio::test(start_paused = true)]
async fn a_silent_rail_center_is_given_up_on_after_5_s() {
    let server = serve(|_| Reply::Silent).await;

    let error = bundles(&server.url).fetch().await.unwrap_err();

    assert_eq!(error.to_string(), "Rail Center was silent for 5s");
}

#[tokio::test(start_paused = true)]
async fn a_dribbling_answer_is_given_up_on_after_30_s() {
    let server =
        serve(|_| Reply::Dribble(b"HTTP/1.1 200 OK\r\n\r\n".to_vec(), Duration::from_secs(1)))
            .await;

    let error = bundles(&server.url).fetch().await.unwrap_err();

    assert_eq!(error.to_string(), "the fetch ran past its 30s deadline");
}
