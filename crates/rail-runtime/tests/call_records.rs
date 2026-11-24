use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;

use crate::call_record_center::{Center, accepted, calls, dropped, window};
use crate::holder::{TestClock, until};
use crate::scripted::{answer, failed};

#[tokio::test(start_paused = true)]
async fn a_window_is_sent_as_its_document_with_a_random_id() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock);
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    reporter.report(window(8, &["/a"]));
    until(&clock, 1).await;

    let documents = center.documents();
    assert_eq!(documents.len(), 2);
    for document in &documents {
        let id: uuid::Uuid = document["document_id"].as_str().unwrap().parse().unwrap();
        assert_eq!(id.get_version_num(), 4, "{document}");
        assert_eq!(
            document["generated_at"], "2027-01-15T08:00:00Z",
            "{document}"
        );
        assert_eq!(calls(document), 1, "{document}");
    }
    assert_ne!(documents[0]["document_id"], documents[1]["document_id"]);
}

#[tokio::test(start_paused = true)]
async fn a_failed_send_is_retried_with_the_same_bytes_backing_off_until_one_is_taken() {
    let clock = TestClock::new();
    let answers = [
        || answer(StatusCode::SERVICE_UNAVAILABLE, b""),
        failed,
        || answer(StatusCode::UNAUTHORIZED, b""),
        || answer(StatusCode::TOO_MANY_REQUESTS, b""),
        || answer(StatusCode::OK, b""),
        accepted,
    ];
    let center = Center::new(&clock, move |n| answers[n]());
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    until(&clock, 100).await;
    reporter.report(window(8, &["/b"]));
    until(&clock, 101).await;

    // A success resets the backoff: the next window goes at once.
    assert_eq!(center.sent_at(), [0, 5, 15, 35, 75, 100]);
    let bodies = center.bodies();
    assert!(bodies[..5].iter().all(|body| *body == bodies[0]));
    assert_ne!(bodies[5], bodies[0]);
}

#[tokio::test(start_paused = true)]
async fn a_refused_document_is_dropped_and_counted_in_the_next() {
    let clock = TestClock::new();
    let center = Center::new(&clock, |n| match n {
        0 => answer(StatusCode::NOT_FOUND, b""),
        _ => accepted(),
    });
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    reporter.report(window(8, &["/b"]));
    until(&clock, 1).await;

    let dropped: Vec<_> = center.documents().iter().map(dropped).collect();
    assert_eq!(dropped, [0, 1]);
    assert_eq!(center.sent_at(), [0, 0]);
}

#[tokio::test(start_paused = true)]
async fn past_the_plans_limit_the_oldest_empty_window_goes_first_then_the_oldest() {
    let clock = TestClock::new();
    let center = Center::holding(&clock);
    let reporter = center.start(&clock);

    // The first in flight stays; past 2, the empty third goes, then the second.
    reporter.report(window(2, &["/a"]));
    until(&clock, 1).await;
    reporter.report(window(2, &["/b"]));
    reporter.report(window(2, &[]));
    reporter.report(window(2, &["/c", "/c"]));
    until(&clock, 2).await;
    center.release.add_permits(2);
    until(&clock, 3).await;

    let windows: Vec<_> = center
        .documents()
        .iter()
        .map(|document| (calls(document), dropped(document)))
        .collect();
    assert_eq!(windows, [(1, 0), (2, 2)]);
}

#[tokio::test(start_paused = true)]
async fn a_document_dropped_after_a_failure_hands_on_the_drops_it_carried() {
    let clock = TestClock::new();
    let center = Center {
        // The second send, the third window's, carrying the second's drop, fails.
        answers: Arc::new(|n| if n == 1 { failed() } else { accepted() }),
        ..Center::holding(&clock)
    };
    let reporter = center.start(&clock);

    // The first in flight, the second dropped past the limit of 1.
    reporter.report(window(1, &["/a"]));
    until(&clock, 1).await;
    reporter.report(window(1, &["/b"]));
    until(&clock, 2).await;
    center.release.add_permits(3);
    until(&clock, 3).await;
    reporter.report(window(1, &["/c"]));
    until(&clock, 4).await;
    // The third, waiting out its backoff, is dropped by the fourth, which carries both drops and
    // waits out the rest of it.
    reporter.report(window(1, &["/d"]));
    until(&clock, 9).await;

    let dropped: Vec<_> = center.documents().iter().map(dropped).collect();
    assert_eq!(dropped, [0, 1, 2]);
    assert_eq!(center.sent_at(), [0, 3, 8]);
}

#[tokio::test(start_paused = true)]
async fn sending_now_skips_the_backoff() {
    let clock = TestClock::new();
    let center = Center::new(&clock, |n| match n {
        0 => failed(),
        _ => accepted(),
    });
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    until(&clock, 1).await;
    reporter.send_now();
    until(&clock, 2).await;

    assert_eq!(center.sent_at(), [0, 1]);
}

#[tokio::test(start_paused = true)]
async fn the_flush_tries_each_once_without_backing_off() {
    let clock = TestClock::new();
    let center = Center::new(&clock, |_| failed());
    let reporter = center.start(&clock);

    for path in ["/a", "/b", "/c"] {
        reporter.report(window(8, &[path]));
    }
    // The first fails, and would wait 5 s.
    until(&clock, 1).await;
    reporter.flush().await;

    assert_eq!(center.sent_at(), [0, 1, 1, 1]);
}

#[tokio::test(start_paused = true)]
async fn the_flush_ends_after_2_s_whatever_is_in_flight() {
    let clock = TestClock::new();
    let center = Center {
        takes: Duration::from_secs(3),
        ..Center::accepting(&clock)
    };
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    reporter.report(window(8, &["/b"]));
    reporter.flush().await;

    assert_eq!(clock.started.elapsed(), Duration::from_secs(2));
    assert_eq!(center.sent_at(), [0]);
}
