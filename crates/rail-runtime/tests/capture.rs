use serde_json::Value;

use crate::call_record_center::{Center, accepted, calls};
use crate::capture_plans::no_plan;
use crate::captures::{capture, get, held};
use crate::holder::{TestClock, until};
use crate::scripted::failed;

fn span(document: &Value) -> (&str, &str) {
    let window = &document["window"];
    (
        window["start"].as_str().unwrap(),
        window["end"].as_str().unwrap(),
    )
}

#[tokio::test(start_paused = true)]
async fn a_window_ends_on_its_boundary_and_is_sent() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock);
    let (_capture, sender) = capture(&clock, |_| held(), &center);

    until(&clock, 1).await;
    assert!(sender.is_capturing());
    sender.send(get());
    sender.send(get());
    until(&clock, 901).await;

    let [document] = &center.documents()[..] else {
        panic!("{:?}", center.documents())
    };
    assert_eq!(
        span(document),
        ("2027-01-15T08:00:00Z", "2027-01-15T08:15:00Z")
    );
    assert_eq!(calls(document), 2, "{document}");
}

#[tokio::test(start_paused = true)]
async fn with_no_plan_held_nothing_is_captured() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock);
    let (capture, sender) = capture(&clock, |_| no_plan(), &center);

    until(&clock, 1).await;
    assert!(!sender.is_capturing());
    sender.send(get());
    until(&clock, 901).await;
    capture.stop().await;

    assert!(center.documents().is_empty(), "{:?}", center.documents());
}

#[tokio::test(start_paused = true)]
async fn the_plan_gone_sends_what_is_queued_at_once() {
    let clock = TestClock::new();
    // The first window's sends fail at 900, 905, 915 and 935; the next would be at 975.
    let center = Center::new(&clock, |n| if n < 4 { failed() } else { accepted() });
    // Fetched every minute: the plan until 900, none at 960.
    let (_capture, sender) = capture(&clock, |n| if n < 16 { held() } else { no_plan() }, &center);

    until(&clock, 1).await;
    sender.send(get());
    until(&clock, 901).await;
    sender.send(get());
    until(&clock, 961).await;

    assert_eq!(center.sent_at(), [900, 905, 915, 935, 960, 960]);
    let ends: Vec<_> = center
        .documents()
        .iter()
        .map(|d| span(d).1.to_owned())
        .collect();
    assert_eq!(ends[4..], ["2027-01-15T08:15:00Z", "2027-01-15T08:16:00Z"]);
}

#[tokio::test(start_paused = true)]
async fn a_call_the_full_queue_turns_away_is_counted_in_the_window() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock);
    let (_capture, sender) = capture(&clock, |_| held(), &center);

    until(&clock, 1).await;
    // Handed over before the task takes any: one past the queue's 4 096.
    for _ in 0..4_097 {
        sender.send(get());
    }
    until(&clock, 901).await;

    let [document] = &center.documents()[..] else {
        panic!("{:?}", center.documents())
    };
    assert_eq!(calls(document), 4_096);
    assert_eq!(document["truncated"]["calls_dropped"], 1, "{document}");
}

#[tokio::test(start_paused = true)]
async fn stopping_closes_the_window_and_sends_what_is_queued_at_once() {
    let clock = TestClock::new();
    // The first window's send fails at 900; the next would be at 905.
    let center = Center::new(&clock, |n| if n == 0 { failed() } else { accepted() });
    let (capture, sender) = capture(&clock, |_| held(), &center);

    until(&clock, 1).await;
    sender.send(get());
    until(&clock, 901).await;
    sender.send(get());
    until(&clock, 902).await;
    capture.stop().await;

    assert_eq!(center.sent_at(), [900, 902, 902]);
    let last = &center.documents()[2];
    assert_eq!(span(last), ("2027-01-15T08:15:00Z", "2027-01-15T08:15:02Z"));
    assert_eq!(calls(last), 1, "{last}");
}
