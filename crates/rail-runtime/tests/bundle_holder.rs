use std::sync::{Arc, Mutex};
use std::time::Duration;

use rail_policy::{EnforcementMode, Fallback};
use rail_runtime::{BundleHolder, FetchError, Jitter, RailCenterBundles, RailCenterThread};
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
            "a 404 carrying a good bundle",
            answer(
                StatusCode::NOT_FOUND,
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
async fn a_first_fetch_is_held() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| ok(&bundle("v1", json!({}))));

    let holder = start(&script, MINUTE);
    until(&clock, 0).await;

    let held = holder.snapshot().unwrap();
    assert_eq!(held.content_hash, "v1");
    assert_eq!(
        (held.schema_version.major, held.schema_version.minor),
        (2, 0)
    );
    assert_eq!(held.posture.mode, EnforcementMode::Observe);
    assert_eq!(held.posture.binding_fallback, Fallback::Block);
    assert_eq!(held.posture.ticket_fallback, Fallback::Pass);
    assert_eq!(script.fetched_at(), [0]);
}

#[tokio::test(start_paused = true)]
async fn nothing_is_held_before_the_first_fetch() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| ok(&bundle("v1", json!({}))));

    let holder = start(&script, MINUTE);

    assert!(holder.snapshot().is_none());
}

#[tokio::test(start_paused = true)]
async fn no_usable_answer_replaces_the_bundle_held() {
    for (label, answer) in unusable_answers() {
        let clock = Clock::new();
        let answer = Mutex::new(Some(answer));
        let script = Script::new(&clock, move |n| {
            if n == 0 {
                ok(&bundle("v1", json!({})))
            } else {
                answer.lock().unwrap().take().unwrap_or_else(failed)
            }
        });

        let holder = start(&script, MINUTE);
        until(&clock, 60).await;

        assert_eq!(script.fetched_at(), [0, 60], "{label}");
        assert_eq!(held_hash(&holder).as_deref(), Some("v1"), "{label}");
    }
}

#[tokio::test(start_paused = true)]
async fn no_usable_first_answer_holds_anything() {
    for (label, answer) in unusable_answers() {
        let clock = Clock::new();
        let answer = Mutex::new(Some(answer));
        let script = Script::new(&clock, move |_| {
            answer.lock().unwrap().take().unwrap_or_else(failed)
        });

        let holder = start(&script, MINUTE);
        until(&clock, 0).await;

        assert_eq!(script.fetched_at(), [0], "{label}");
        assert!(holder.snapshot().is_none(), "{label}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_higher_minor_is_held_like_any_other_bundle() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| {
        ok(&bundle("v9", json!({"schema_version": "2.7"})))
    });

    let holder = start(&script, MINUTE);
    until(&clock, 0).await;

    let held = holder.snapshot().unwrap();
    assert_eq!(
        (held.schema_version.major, held.schema_version.minor),
        (2, 7)
    );
}

#[tokio::test(start_paused = true)]
async fn a_changed_bundle_replaces_the_one_held() {
    let clock = Clock::new();
    let script = Script::new(&clock, |n| {
        if n == 0 {
            ok(&bundle("v1", json!({})))
        } else {
            ok(&bundle("v2", json!({"enforcement": {"mode": "enforce"}})))
        }
    });

    let holder = start(&script, MINUTE);
    until(&clock, 60).await;

    let held = holder.snapshot().unwrap();
    assert_eq!(held.content_hash, "v2");
    assert_eq!(held.posture.mode, EnforcementMode::Enforce);
}

#[tokio::test(start_paused = true)]
async fn the_same_content_hash_is_not_read_again() {
    let clock = Clock::new();
    // Refused if it were read: the hash alone says it's the bundle held.
    let script = Script::new(&clock, |n| {
        if n == 0 {
            ok(&bundle("v1", json!({})))
        } else {
            ok(&bundle("v1", json!({"policies": "not a list"})))
        }
    });

    let holder = start(&script, MINUTE);
    until(&clock, 0).await;
    let first = holder.snapshot().unwrap();
    until(&clock, 60).await;

    assert!(Arc::ptr_eq(&first, &holder.snapshot().unwrap()));
    assert_eq!(script.fetched_at(), [0, 60]);
}

#[tokio::test(start_paused = true)]
async fn a_bundle_is_refetched_every_interval() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| ok(&bundle("v1", json!({}))));

    let _holder = start(&script, MINUTE);
    until(&clock, 180).await;

    assert_eq!(script.fetched_at(), [0, 60, 120, 180]);
}

#[tokio::test(start_paused = true)]
async fn a_failure_backs_off_from_5_s_up_to_the_interval_and_a_success_resumes_it() {
    let clock = Clock::new();
    let script = Script::new(&clock, |n| {
        if n < 6 {
            failed()
        } else {
            ok(&bundle("v1", json!({})))
        }
    });

    let holder = start(&script, MINUTE);
    until(&clock, 255).await;

    // 5, 10, 20, 40, then the interval; held at 195, and a minute later again.
    assert_eq!(script.fetched_at(), [0, 5, 15, 35, 75, 135, 195, 255]);
    assert_eq!(held_hash(&holder).as_deref(), Some("v1"));
}

#[tokio::test(start_paused = true)]
async fn an_interval_under_5_s_is_raised_to_it() {
    let clock = Clock::new();
    // Back-to-back fetches would never let paused time advance, so fail rather than hang.
    let script = Script::new(&clock, |n| {
        assert!(n < 10, "fetching back to back");
        ok(&bundle("v1", json!({})))
    });

    let _holder = start(&script, Duration::ZERO);
    until(&clock, 12).await;

    assert_eq!(script.fetched_at(), [0, 5, 10]);
}

#[tokio::test(start_paused = true)]
async fn dropping_the_holder_stops_the_refreshes() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| ok(&bundle("v1", json!({}))));

    let holder = start(&script, MINUTE);
    until(&clock, 60).await;
    drop(holder);
    until(&clock, 600).await;

    assert_eq!(script.fetched_at(), [0, 60]);
}

#[tokio::test(start_paused = true)]
async fn dropping_the_holder_during_a_fetch_stops_the_refreshes() {
    let clock = Clock::new();
    let script = Script {
        takes: Duration::from_secs(10),
        ..Script::new(&clock, |_| ok(&bundle("v1", json!({}))))
    };

    let holder = start(&script, MINUTE);
    until(&clock, 5).await;
    drop(holder);
    until(&clock, 600).await;

    assert_eq!(script.fetched_at(), [0]);
}

// Waiting for a bundle.

// What a wait for a bundle returns, and how long it took.
async fn timed_wait(holder: &BundleHolder) -> (Option<String>, Duration) {
    let started = Instant::now();
    let held = holder.wait_for_bundle(LIMIT).await;
    (
        held.map(|bundle| bundle.content_hash.clone()),
        started.elapsed(),
    )
}

#[tokio::test(start_paused = true)]
async fn a_wait_with_a_bundle_held_returns_it_at_once() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| ok(&bundle("v1", json!({}))));

    let holder = start(&script, MINUTE);
    until(&clock, 10).await;

    assert_eq!(
        timed_wait(&holder).await,
        (Some("v1".to_owned()), Duration::ZERO)
    );
    assert_eq!(script.fetched_at(), [0]);
}

#[tokio::test(start_paused = true)]
async fn a_wait_during_the_first_fetch_takes_its_answer() {
    let clock = Clock::new();
    let script = Script {
        takes: Duration::from_secs(1),
        ..Script::new(&clock, |_| ok(&bundle("v1", json!({}))))
    };

    let holder = start(&script, MINUTE);

    assert_eq!(
        timed_wait(&holder).await,
        (Some("v1".to_owned()), Duration::from_secs(1))
    );
}

#[tokio::test(start_paused = true)]
async fn a_wait_gives_up_at_its_limit() {
    let clock = Clock::new();
    let script = Script {
        takes: Duration::from_secs(10),
        ..Script::new(&clock, |_| ok(&bundle("v1", json!({}))))
    };

    let holder = start(&script, MINUTE);

    assert_eq!(timed_wait(&holder).await, (None, LIMIT));
}

#[tokio::test(start_paused = true)]
async fn a_wait_waits_for_a_retry_due_within_its_limit() {
    let clock = Clock::new();
    let script = Script::new(&clock, |n| {
        if n == 0 {
            failed()
        } else {
            ok(&bundle("v1", json!({})))
        }
    });

    let holder = start(&script, MINUTE);
    until(&clock, 4).await;

    // The retry is due at 5, within the 2 s.
    assert_eq!(
        timed_wait(&holder).await,
        (Some("v1".to_owned()), Duration::from_secs(1))
    );
}

#[tokio::test(start_paused = true)]
async fn a_wait_returns_at_once_when_no_attempt_is_due_within_its_limit_and_starts_none() {
    let clock = Clock::new();
    let script = Script::new(&clock, |n| {
        if n == 0 {
            failed()
        } else {
            ok(&bundle("v1", json!({})))
        }
    });

    let holder = start(&script, MINUTE);
    until(&clock, 1).await;

    // The retry is due at 5, past the 2 s.
    assert_eq!(timed_wait(&holder).await, (None, Duration::ZERO));
    assert_eq!(script.fetched_at(), [0]);
}

#[tokio::test(start_paused = true)]
async fn a_wait_returns_at_once_for_a_retry_due_just_as_its_limit_passes() {
    let clock = Clock::new();
    let script = Script::new(&clock, |n| {
        if n == 0 {
            failed()
        } else {
            ok(&bundle("v1", json!({})))
        }
    });

    let holder = start(&script, MINUTE);
    until(&clock, 3).await;

    // The retry is due at 5, as the 2 s end: it can't land in time.
    assert_eq!(timed_wait(&holder).await, (None, Duration::ZERO));
}

#[tokio::test(start_paused = true)]
async fn a_wait_stops_once_a_failed_retry_leaves_none_due_within_its_limit() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| failed());

    let holder = start(&script, MINUTE);
    until(&clock, 4).await;

    // The retry at 5 fails, and the next is due at 15.
    assert_eq!(timed_wait(&holder).await, (None, Duration::from_secs(1)));
    assert_eq!(script.fetched_at(), [0, 5]);
}

#[tokio::test(start_paused = true)]
async fn concurrent_waits_share_the_fetch_in_flight() {
    let clock = Clock::new();
    let script = Script {
        takes: Duration::from_secs(1),
        ..Script::new(&clock, |_| ok(&bundle("v1", json!({}))))
    };

    let holder = start(&script, MINUTE);
    let waits = tokio::join!(
        timed_wait(&holder),
        timed_wait(&holder),
        timed_wait(&holder)
    );

    let waited = (Some("v1".to_owned()), Duration::from_secs(1));
    assert_eq!(waits, (waited.clone(), waited.clone(), waited));
    assert_eq!(script.fetched_at(), [0]);
}

// Real time: the holder's loop runs on the Rail Center thread's runtime, the wait on this one.
#[tokio::test]
async fn a_wait_on_another_runtime_is_woken_by_the_loop() {
    let thread = RailCenterThread::start().unwrap();
    let clock = Clock::new();
    let script = Script {
        takes: Duration::from_millis(100),
        ..Script::new(&clock, |_| ok(&bundle("v1", json!({}))))
    };
    let holder = BundleHolder::start(
        thread.runtime(),
        script.clone(),
        MINUTE,
        Jitter::new(|| 1.0),
    );

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
