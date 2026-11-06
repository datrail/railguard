use std::time::Duration;

use rail_runtime::{BundleHolder, FetchError, Jitter, RailCenterBundles, RailCenterThread};
use reqwest::StatusCode;
use serde_json::json;
use tokio::runtime::Handle;
use tokio::time::Instant;

use crate::bundles::{Answer, MINUTE, Script, answer, bundle, failed, held_hash, ok, start};
use crate::common::{self, Reply, client, response, serve};
use crate::holder::{TestClock, until};
use crate::scripted::Answering;

const LIMIT: Duration = Duration::from_secs(2);

// Every way of getting no usable bundle, a bundle Rail Center sent or not.
fn unusable_answers() -> Vec<(&'static str, Answering)> {
    fn without(field: &str) -> Answer {
        let mut body = bundle("v2", json!({}));
        body.as_object_mut().unwrap().remove(field);
        ok(&body)
    }
    vec![
        ("a 503", || answer(StatusCode::SERVICE_UNAVAILABLE, b"")),
        ("a 500", || answer(StatusCode::INTERNAL_SERVER_ERROR, b"")),
        ("a 404", || answer(StatusCode::NOT_FOUND, b"")),
        ("a refused credential", || {
            answer(StatusCode::UNAUTHORIZED, b"")
        }),
        ("a forbidden read", || answer(StatusCode::FORBIDDEN, b"")),
        ("a connection failure", || {
            Err(FetchError::Transport("connection refused".to_owned()))
        }),
        ("a deadline", failed),
        ("a body that isn't JSON", || {
            answer(StatusCode::OK, b"<html>nope</html>")
        }),
        ("an empty body", || answer(StatusCode::OK, b"")),
        ("a body that isn't UTF-8", || {
            answer(StatusCode::OK, b"{\"\xff\":1}")
        }),
        ("a 202 carrying a good bundle", || {
            answer(
                StatusCode::ACCEPTED,
                bundle("v2", json!({})).to_string().as_bytes(),
            )
        }),
        ("a JSON array", || ok(&json!([]))),
        ("another major", || {
            ok(&bundle("v2", json!({"schema_version": "3.0"})))
        }),
        ("a version not MAJOR.MINOR", || {
            ok(&bundle("v2", json!({"schema_version": "2.0.1"})))
        }),
        ("policies not a list", || {
            ok(&bundle("v2", json!({"policies": "not a list"})))
        }),
        ("no posture", || without("enforcement")),
        ("a posture outside the contract", || {
            ok(&bundle("v2", json!({"enforcement": {"mode": "audit"}})))
        }),
        ("no content_hash", || without("content_hash")),
    ]
}

#[tokio::test(start_paused = true)]
async fn only_a_usable_answer_is_held_and_any_other_is_retried_as_a_failure() {
    for (label, unusable) in unusable_answers() {
        let clock = TestClock::new();
        let script = Script::new(&clock, move |n| match n {
            1 => ok(&bundle("v1", json!({}))),
            _ => unusable(),
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
async fn the_issuer_and_audience_are_held_only_as_strings() {
    let named = json!({"ticket_issuer": "https://rc/tenants/t", "ticket_audience": "t"});
    for (label, with, held) in [
        ("named", named, (Some("https://rc/tenants/t"), Some("t"))),
        ("null", json!({}), (None, None)),
        (
            "not a string",
            json!({"ticket_issuer": 1, "ticket_audience": true}),
            (None, None),
        ),
    ] {
        let clock = TestClock::new();
        let script = Script::new(&clock, move |_| ok(&bundle("v1", with.clone())));

        let holder = start(&script, MINUTE);
        until(&clock, 0).await;

        let bundle = holder.snapshot().unwrap();
        let read = (
            bundle.ticket_issuer.as_deref(),
            bundle.ticket_audience.as_deref(),
        );
        assert_eq!(read, held, "{label}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_wait_ends_once_no_attempt_can_land_within_its_limit() {
    // The retry is due at 5: asked at 3 it can't land in time, asked at 4 it fails, the next at 15.
    for (asked_at, waited, fetched_at) in [
        (3, Duration::ZERO, [0].as_slice()),
        (4, Duration::from_secs(1), &[0, 5]),
    ] {
        let clock = TestClock::new();
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

#[tokio::test(start_paused = true)]
async fn a_wait_during_a_fetch_takes_its_answer_or_gives_up_at_its_limit() {
    // A fetch of 1 s lands within the 2 s; one of 10 s doesn't.
    for (takes, held, waited) in [(1, Some("v1"), 1), (10, None, 2)] {
        let clock = TestClock::new();
        let script = Script {
            takes: Duration::from_secs(takes),
            ..Script::new(&clock, |_| ok(&bundle("v1", json!({}))))
        };

        let holder = start(&script, MINUTE);
        let started = Instant::now();
        let bundle = holder.wait_for_bundle(LIMIT).await;

        let hash = bundle.map(|bundle| bundle.content_hash.clone());
        assert_eq!(hash.as_deref(), held, "{takes}");
        assert_eq!(started.elapsed(), Duration::from_secs(waited), "{takes}");
        assert_eq!(script.fetched_at(), [0], "{takes}");
    }
}

// Real time: the holder's loop runs on the Rail Center thread's runtime, the wait on this one.
#[tokio::test]
async fn a_wait_on_another_runtime_is_woken_by_the_loop() {
    let thread = RailCenterThread::start().unwrap();
    let clock = TestClock::new();
    let script = Script {
        takes: Duration::from_millis(100),
        ..Script::new(&clock, |_| ok(&bundle("v1", json!({}))))
    };
    let jitter = Jitter::new(|| 1.0);
    let holder = BundleHolder::start(thread.runtime(), script.clone(), MINUTE, jitter);

    let held = holder.wait_for_bundle(Duration::from_secs(10)).await;

    assert_eq!(held.unwrap().content_hash, "v1");
    assert_eq!(script.fetched_at().len(), 1);
    drop(holder);
    tokio::task::spawn_blocking(move || thread.stop())
        .await
        .unwrap();
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
