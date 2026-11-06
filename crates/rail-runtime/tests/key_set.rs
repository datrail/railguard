use std::sync::Mutex;
use std::time::Duration;

use rail_runtime::{Jitter, KeySetHolder, KeySource, RailCenterKeys, RailCenterThread};
use reqwest::StatusCode;
use tokio::time::Instant;

use crate::common::{Reply, TOKEN, client, response, serve};
use crate::holder::{Clock, until};
use crate::keys::{
    AgingClock, Answer, LIMIT, Script, answer, failed, key_set, kids, named, ok, start, start_aging,
};

// What waiting for `kid` returns, and how long it took.
async fn timed_wait(holder: &KeySetHolder, kid: &str) -> (Option<Vec<String>>, Duration) {
    let started = Instant::now();
    let set = holder.wait_for_kid(kid, LIMIT).await;
    let mut kids: Option<Vec<String>> = set.map(|set| set.keys.keys().cloned().collect());
    if let Some(kids) = &mut kids {
        kids.sort();
    }
    (kids, started.elapsed())
}

#[tokio::test(start_paused = true)]
async fn a_fetched_set_is_held() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| ok(&["a"]));

    let holder = start(&clock, &script);
    until(&clock, 0).await;

    assert_eq!(kids(&holder), named(&["a"]));
    assert_eq!(script.fetched_at(), [0]);
}

#[tokio::test(start_paused = true)]
async fn nothing_is_held_before_the_first_fetch() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| ok(&["a"]));

    let holder = start(&clock, &script);

    assert_eq!(kids(&holder), None);
}

// Every way of getting no usable set.
fn unusable_answers() -> Vec<(&'static str, Answer)> {
    vec![
        ("a 503", answer(StatusCode::SERVICE_UNAVAILABLE, b"")),
        ("a 404", answer(StatusCode::NOT_FOUND, b"")),
        (
            "a refused credential",
            answer(StatusCode::UNAUTHORIZED, b""),
        ),
        ("a forbidden read", answer(StatusCode::FORBIDDEN, b"")),
        (
            "a 404 carrying a set",
            answer(
                StatusCode::NOT_FOUND,
                key_set(&["b"]).to_string().as_bytes(),
            ),
        ),
        (
            "a 203 carrying a set",
            answer(
                StatusCode::NON_AUTHORITATIVE_INFORMATION,
                key_set(&["b"]).to_string().as_bytes(),
            ),
        ),
        ("a body that isn't JSON", answer(StatusCode::OK, b"<html>")),
        ("no usable key", answer(StatusCode::OK, b"{\"keys\": []}")),
        ("a kid twice", ok(&["b", "b"])),
        ("no response", failed()),
    ]
}

#[tokio::test(start_paused = true)]
async fn no_usable_answer_replaces_the_set_held_nor_holds_one_first() {
    let labels: Vec<&str> = unusable_answers().iter().map(|(label, _)| *label).collect();
    for (at, label) in labels.into_iter().enumerate() {
        // The answer for `label`, once, then failures.
        let unusable = || {
            let answer = Mutex::new(Some(unusable_answers().remove(at).1));
            move || answer.lock().unwrap().take().unwrap_or_else(failed)
        };
        let clock = Clock::new();
        let after_one = unusable();
        let warm = Script::new(
            &clock,
            move |n| if n == 0 { ok(&["a"]) } else { after_one() },
        );
        let first = unusable();
        let cold = Script::new(&clock, move |_| first());

        let warm_holder = start(&clock, &warm);
        let cold_holder = start(&clock, &cold);
        until(&clock, 300).await;

        assert_eq!(warm.fetched_at(), [0, 300], "{label}");
        assert_eq!(kids(&warm_holder), named(&["a"]), "{label}");
        assert_eq!(kids(&cold_holder), None, "{label}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_new_set_replaces_the_whole_set() {
    let clock = Clock::new();
    let script = Script::new(
        &clock,
        |n| if n == 0 { ok(&["a", "b"]) } else { ok(&["c"]) },
    );

    let holder = start(&clock, &script);
    until(&clock, 300).await;

    assert_eq!(kids(&holder), named(&["c"]));
}

#[tokio::test(start_paused = true)]
async fn the_set_is_refetched_every_300_s_and_a_failure_backs_off_up_to_that() {
    let clock = Clock::new();
    let good = Script::new(&clock, |_| ok(&["a"]));
    let bad = Script::new(&clock, |_| failed());

    let _good = start(&clock, &good);
    let _bad = start(&clock, &bad);
    until(&clock, 615).await;

    assert_eq!(good.fetched_at(), [0, 300, 600]);
    assert_eq!(bad.fetched_at(), [0, 5, 15, 35, 75, 155, 315, 615]);
}

#[tokio::test(start_paused = true)]
async fn a_set_is_held_900_s_after_the_last_good_fetch_and_no_longer() {
    let clock = Clock::new();
    let script = Script::new(&clock, |n| if n == 0 { ok(&["a"]) } else { failed() });

    let holder = start(&clock, &script);
    until(&clock, 900).await;
    assert_eq!(kids(&holder), named(&["a"]));
    until(&clock, 901).await;

    assert_eq!(kids(&holder), None);
}

#[tokio::test(start_paused = true)]
async fn time_the_host_spent_suspended_ages_the_set() {
    let clock = Clock::new();
    let aging = AgingClock::new(&clock);
    let script = Script::new(&clock, |_| ok(&["a"]));

    let holder = start_aging(&aging, &script);
    until(&clock, 10).await;
    aging.skip(Duration::from_secs(900));

    assert_eq!(kids(&holder), None);
}

// An unknown kid.

#[tokio::test(start_paused = true)]
async fn an_unknown_kid_is_refetched_and_waited_for() {
    let clock = Clock::new();
    let script = Script {
        takes: Duration::from_secs(1),
        ..Script::new(
            &clock,
            |n| if n == 0 { ok(&["a"]) } else { ok(&["a", "b"]) },
        )
    };

    let holder = start(&clock, &script);
    until(&clock, 40).await;

    assert_eq!(
        timed_wait(&holder, "b").await,
        (named(&["a", "b"]), Duration::from_secs(1))
    );
    assert_eq!(script.fetched_at(), [0, 40]);
}

#[tokio::test(start_paused = true)]
async fn a_held_kid_waits_for_nothing() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| ok(&["a"]));

    let holder = start(&clock, &script);
    until(&clock, 40).await;

    assert_eq!(
        timed_wait(&holder, "a").await,
        (named(&["a"]), Duration::ZERO)
    );
    assert_eq!(script.fetched_at(), [0]);
}

#[tokio::test(start_paused = true)]
async fn unknown_kids_refetch_at_most_once_per_30_s_whatever_the_kid() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| ok(&["a"]));

    let holder = start(&clock, &script);
    // 10 s after the scheduled fetch, 10 and 29 after a forced one, then 30 after it.
    for (at, kid) in [(10, "b"), (40, "b"), (50, "c"), (69, "e"), (70, "d")] {
        until(&clock, at).await;
        assert_eq!(
            timed_wait(&holder, kid).await,
            (named(&["a"]), Duration::ZERO)
        );
    }

    assert_eq!(script.fetched_at(), [0, 40, 70]);
}

#[tokio::test(start_paused = true)]
async fn the_30_s_count_from_when_the_last_attempt_started() {
    let clock = Clock::new();
    let script = Script {
        takes: Duration::from_secs(5),
        ..Script::new(&clock, |_| ok(&["a"]))
    };

    let holder = start(&clock, &script);
    // The first attempt ran from 0 to 5: 30 s after it started, 25 after it ended.
    until(&clock, 30).await;
    holder.wait_for_kid("b", LIMIT).await;

    assert_eq!(script.fetched_at(), [0, 30]);
}

#[tokio::test(start_paused = true)]
async fn concurrent_waits_share_one_refetch() {
    let clock = Clock::new();
    let script = Script {
        takes: Duration::from_secs(1),
        ..Script::new(&clock, |n| {
            if n == 0 {
                ok(&["a"])
            } else {
                ok(&["a", "b", "c"])
            }
        })
    };

    let holder = start(&clock, &script);
    until(&clock, 40).await;
    let waits = tokio::join!(
        timed_wait(&holder, "b"),
        timed_wait(&holder, "c"),
        timed_wait(&holder, "b")
    );

    let waited = (named(&["a", "b", "c"]), Duration::from_secs(1));
    assert_eq!(waits, (waited.clone(), waited.clone(), waited));
    assert_eq!(script.fetched_at(), [0, 40]);
}

#[tokio::test(start_paused = true)]
async fn a_wait_for_a_kid_takes_the_answer_of_an_attempt_in_flight() {
    let clock = Clock::new();
    let script = Script {
        takes: Duration::from_secs(1),
        ..Script::new(
            &clock,
            |n| if n == 0 { ok(&["a"]) } else { ok(&["a", "b"]) },
        )
    };

    let holder = start(&clock, &script);
    until(&clock, 300).await;

    assert_eq!(
        timed_wait(&holder, "b").await,
        (named(&["a", "b"]), Duration::from_secs(1))
    );
    assert_eq!(script.fetched_at(), [0, 300]);
}

#[tokio::test(start_paused = true)]
async fn with_none_held_an_unknown_kid_refetches_without_waiting_out_the_backoff() {
    let clock = Clock::new();
    let script = Script::new(&clock, |n| if n < 5 { failed() } else { ok(&["b"]) });

    let holder = start(&clock, &script);
    // Failed at 0, 5, 15, 35 and 75, and next due at 155.
    until(&clock, 110).await;

    assert_eq!(
        timed_wait(&holder, "b").await,
        (named(&["b"]), Duration::ZERO)
    );
    assert_eq!(script.fetched_at(), [0, 5, 15, 35, 75, 110]);
}

#[tokio::test(start_paused = true)]
async fn a_refetch_that_brings_nothing_leaves_the_kid_unknown() {
    let clock = Clock::new();
    let script = Script::new(&clock, |n| if n == 0 { ok(&["a"]) } else { failed() });

    let holder = start(&clock, &script);
    until(&clock, 40).await;

    // Its retry is due at 45, past the 2 s.
    assert_eq!(
        timed_wait(&holder, "b").await,
        (named(&["a"]), Duration::ZERO)
    );
    assert_eq!(script.fetched_at(), [0, 40]);
}

#[tokio::test(start_paused = true)]
async fn a_set_past_its_age_is_refetched_for_a_kid_it_lists_and_gives_nothing_if_that_fails() {
    let clock = Clock::new();
    let script = Script::new(&clock, |n| if n == 0 { ok(&["a"]) } else { failed() });

    let holder = start(&clock, &script);
    // Aged out at 901; the last attempt, at 915, failed, and the next is due at 1215.
    until(&clock, 950).await;

    assert_eq!(timed_wait(&holder, "a").await, (None, Duration::ZERO));
    assert_eq!(script.fetched_at().last(), Some(&950));
}

// Real time: the loop runs on the Rail Center thread's runtime, stopped under the waits.
#[tokio::test]
async fn once_the_loop_is_gone_a_wait_returns_at_once() {
    let thread = RailCenterThread::start().unwrap();
    let clock = Clock::new();
    let script = Script {
        takes: Duration::from_secs(3600),
        ..Script::new(&clock, |_| ok(&["a"]))
    };
    let holder = KeySetHolder::start(
        thread.runtime(),
        script,
        rail_runtime::Clock::system(),
        Jitter::new(|| 1.0),
    );
    tokio::task::spawn_blocking(move || thread.stop())
        .await
        .unwrap();

    let started = Instant::now();
    let set = holder.wait_for_kid("a", Duration::from_secs(10)).await;

    assert!(set.is_none());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}

// Real time: the fetch goes over a socket, which paused time would race.
#[tokio::test]
async fn the_route_and_the_credential_travel_with_the_request() {
    let body = key_set(&["a"]).to_string();
    let server = serve(move |_| Reply::Raw(response("200 OK", &[], body.as_bytes()))).await;
    let source = RailCenterKeys {
        client: client(&format!("{}/rc/", server.url)),
    };

    let fetched = source.fetch().await.unwrap();

    assert_eq!(fetched.status, StatusCode::OK);
    let requests = server.requests.lock().unwrap();
    let request = requests[0].to_ascii_lowercase();
    assert!(
        request.starts_with("get /rc/v1/jwks.json http/1.1\r\n"),
        "{request}"
    );
    assert!(
        request.contains(&format!(
            "authorization: bearer {}",
            TOKEN.to_ascii_lowercase()
        )),
        "{request}"
    );
    assert!(request.contains("cache-control: no-cache\r\n"), "{request}");
}
