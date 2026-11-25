use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use rail_runtime::{Capture, CapturePlanHolder, Jitter, RailCenterThread, WallClock};
use tokio::runtime::Handle;

use crate::call_record_center::{Center, accepted, calls};
use crate::capture_plans::{MINUTE, agent, no_plan, start};
use crate::captures::{a_plan, capture, capture_every, to_catch_all};
use crate::holder::{TestClock, until};
use crate::scripted::{Script, failed};

#[tokio::test(start_paused = true)]
async fn a_window_ends_on_its_boundary_and_is_sent() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock);
    let (_capture, sender) = capture(&clock, |_| a_plan(), &center);

    until(&clock, 1).await;
    assert!(sender.is_capturing());
    sender.send(to_catch_all());
    sender.send(to_catch_all());
    // With no sender left, the window still ends.
    drop(sender);
    until(&clock, 901).await;

    assert_eq!(center.sent_at(), [900]);
    assert_eq!(calls(&center.documents()[0]), 2);
}

#[tokio::test(start_paused = true)]
async fn the_plan_gone_sends_what_is_queued_at_once() {
    let clock = TestClock::new();
    // The first window's send fails at 900; the next would be at 905.
    let center = Center::new(&clock, |n| if n == 0 { failed() } else { accepted() });
    // Fetched at 0, 451 and 902, the plan gone at the last, between the task's wakes at 900 and 960.
    let refresh = Duration::from_secs(451);
    let answers = |n| if n < 2 { a_plan() } else { no_plan() };
    let (_capture, sender) = capture_every(&clock, refresh, answers, &center);

    until(&clock, 1).await;
    sender.send(to_catch_all());
    until(&clock, 901).await;
    sender.send(to_catch_all());
    until(&clock, 903).await;

    assert!(!sender.is_capturing());
    assert_eq!(center.sent_at(), [900, 902, 902]);
    assert_eq!(calls(&center.documents()[2]), 1);
}

#[tokio::test(start_paused = true)]
async fn a_call_the_full_queue_turns_away_is_counted_in_the_window() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock);
    let (_capture, sender) = capture(&clock, |_| a_plan(), &center);

    until(&clock, 1).await;
    // Handed over before the task takes any, well past the queue's 4 096.
    for _ in 0..5_000 {
        sender.send(to_catch_all());
    }
    until(&clock, 901).await;

    let document = &center.documents()[0];
    let dropped = document["truncated"]["calls_dropped"].as_u64().unwrap();
    assert!(dropped > 0, "{document}");
    assert_eq!(calls(document) + dropped, 5_000);
}

#[tokio::test(start_paused = true)]
async fn stopping_counts_the_calls_handed_over_and_sends_every_window_at_once() {
    let clock = TestClock::new();
    // The first window's send fails at 900; the next would be at 905.
    let center = Center::new(&clock, |n| if n == 0 { failed() } else { accepted() });
    let (capture, sender) = capture(&clock, |_| a_plan(), &center);

    until(&clock, 1).await;
    sender.send(to_catch_all());
    until(&clock, 901).await;
    // Handed over, but not yet taken when the stop comes.
    sender.send(to_catch_all());
    capture.stop().await;

    assert!(!sender.is_capturing());
    assert_eq!(center.sent_at(), [900, 901, 901]);
    assert_eq!(calls(&center.documents()[2]), 1);
}

#[tokio::test(start_paused = true)]
async fn the_plans_refreshes_ending_leaves_the_window_to_its_end() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock);
    // The plan fetched on a thread of its own, in real time, which then stops.
    let rail_center = RailCenterThread::start().unwrap();
    let source = Script {
        agent: agent(),
        ..Script::new(&clock, |_| a_plan())
    };
    let plans =
        CapturePlanHolder::start(rail_center.runtime(), source, MINUTE, Jitter::new(|| 1.0));
    let reporter = center.start(&clock);
    let (_capture, sender) = Capture::start(&Handle::current(), plans, reporter, clock.wall());
    let fetched_by = Instant::now() + Duration::from_secs(10);
    while !sender.is_capturing() {
        assert!(Instant::now() < fetched_by, "the plan never came");
        thread::sleep(Duration::from_millis(5));
    }
    until(&clock, 1).await;
    sender.send(to_catch_all());
    rail_center.stop();
    until(&clock, 901).await;

    assert_eq!(center.sent_at(), [900]);
    assert_eq!(calls(&center.documents()[0]), 1);
}

#[tokio::test(start_paused = true)]
async fn a_clock_stepped_back_is_seen_within_a_minute_though_no_call_comes() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock);
    let plans = start(&Script::new(&clock, |_| a_plan()), MINUTE);
    // Ten minutes back at 130 s, with no call after.
    let wall = clock.wall();
    let started = clock.started;
    let stepped: WallClock = Arc::new(move || {
        let back = Duration::from_secs(if started.elapsed() >= Duration::from_secs(130) {
            600
        } else {
            0
        });
        wall() - back
    });
    let (_capture, sender) =
        Capture::start(&Handle::current(), plans, center.start(&clock), stepped);

    until(&clock, 1).await;
    sender.send(to_catch_all());
    until(&clock, 191).await;

    // Within a minute of the step, not at the window's end, 900 s on.
    let sent_at = center.sent_at();
    assert!(
        matches!(sent_at[..], [at] if (130..=190).contains(&at)),
        "{sent_at:?}"
    );
}
