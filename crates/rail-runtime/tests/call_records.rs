use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use rail_runtime::{CallRecordReporter, CallRecordSink, Jitter, RailCenterCallRecords, WallClock};
use reqwest::StatusCode;
use tokio::runtime::Handle;

use crate::call_record_center::{Center, accepted, calls, dropped, window, windows};
use crate::common::{Reply, assert_posted, client, response, serve};
use crate::holder::{TestClock, until};
use crate::scripted::{answer, failed};

#[tokio::test(start_paused = true)]
async fn a_window_is_sent_as_its_document() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock);
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    until(&clock, 1).await;

    let [document] = &center.documents()[..] else {
        panic!("{:?}", center.documents())
    };
    assert_eq!(
        document["generated_at"], "2027-01-15T08:00:00Z",
        "{document}"
    );
    assert_eq!(calls(document), 1, "{document}");
}

#[tokio::test(start_paused = true)]
async fn a_failed_send_is_retried_with_the_same_bytes_backing_off_until_one_is_taken() {
    let clock = TestClock::new();
    let answers = [
        || answer(StatusCode::SERVICE_UNAVAILABLE, b""),
        failed,
        || answer(StatusCode::UNAUTHORIZED, b""),
        accepted,
        failed,
        accepted,
    ];
    let center = Center::new(&clock, move |n| answers[n]());
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    until(&clock, 100).await;
    reporter.report(window(8, &["/b"]));
    until(&clock, 106).await;

    // A success resets the backoff: the next failure waits 5 s again.
    assert_eq!(center.sent_at(), [0, 5, 15, 35, 100, 105]);
    let bodies = center.bodies();
    assert!(bodies[..4].iter().all(|body| *body == bodies[0]));
    let documents = center.documents();
    assert_ne!(documents[4]["document_id"], documents[0]["document_id"]);
}

#[tokio::test(start_paused = true)]
async fn a_refused_document_is_dropped_counted_in_the_next_and_leaves_the_backoff() {
    let clock = TestClock::new();
    let center = Center::new(&clock, |n| match n {
        0 | 2 => failed(),
        1 => answer(StatusCode::NOT_FOUND, b""),
        _ => accepted(),
    });
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    reporter.report(window(8, &["/b"]));
    until(&clock, 16).await;

    // The second goes at once after the refusal, and its failure is the second in a row.
    assert_eq!(center.sent_at(), [0, 5, 5, 15]);
    let dropped: Vec<_> = center.documents().iter().map(dropped).collect();
    assert_eq!(dropped, [0, 0, 1, 1]);
}

#[tokio::test(start_paused = true)]
async fn past_the_plans_limit_the_oldest_empty_window_goes_first() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock).holding();
    let reporter = center.start(&clock);

    // Besides the first, in flight, 2 are kept: the empty third goes, not the older second.
    reporter.report(window(2, &["/a"]));
    until(&clock, 1).await;
    reporter.report(window(2, &["/b"]));
    reporter.report(window(2, &[]));
    reporter.report(window(2, &["/c", "/c"]));
    until(&clock, 2).await;
    center.release.add_permits(3);
    until(&clock, 3).await;

    assert_eq!(windows(&center), [(1, 0), (1, 1), (2, 0)]);
}

#[tokio::test(start_paused = true)]
async fn the_one_in_flight_is_never_dropped() {
    let clock = TestClock::new();
    let center = Center::new(&clock, |n| if n == 0 { failed() } else { accepted() }).holding();
    let reporter = center.start(&clock);

    // With none kept besides it, the second goes; the first, released at 2 s, fails and is sent
    // again 5 s later as it was.
    reporter.report(window(0, &["/a"]));
    until(&clock, 1).await;
    reporter.report(window(0, &["/b"]));
    until(&clock, 2).await;
    center.release.add_permits(2);
    until(&clock, 8).await;

    assert_eq!(center.sent_at(), [0, 7]);
    let bodies = center.bodies();
    assert_eq!(bodies[1], bodies[0]);
}

#[tokio::test(start_paused = true)]
async fn the_newest_windows_plan_sets_the_limit() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock).holding();
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    until(&clock, 1).await;
    reporter.report(window(8, &["/b"]));
    reporter.report(window(8, &["/c"]));
    reporter.report(window(1, &["/d", "/d"]));
    until(&clock, 2).await;
    center.release.add_permits(3);
    until(&clock, 3).await;

    assert_eq!(windows(&center), [(1, 0), (2, 2)]);
}

#[tokio::test(start_paused = true)]
async fn a_document_dropped_after_a_failure_hands_on_the_drops_it_carried() {
    let clock = TestClock::new();
    // The second send, the third window's, carrying the second's drop, fails.
    let center = Center::new(&clock, |n| if n == 1 { failed() } else { accepted() }).holding();
    let reporter = center.start(&clock);

    // With none kept besides the first, in flight, the second is dropped.
    reporter.report(window(0, &["/a"]));
    until(&clock, 1).await;
    reporter.report(window(0, &["/b"]));
    until(&clock, 2).await;
    center.release.add_permits(3);
    until(&clock, 3).await;
    reporter.report(window(0, &["/c"]));
    until(&clock, 4).await;
    // The third, waiting out its backoff, is dropped by the fourth, which carries both drops and
    // waits out the rest of it.
    reporter.report(window(0, &["/d"]));
    until(&clock, 9).await;

    let dropped: Vec<_> = center.documents().iter().map(dropped).collect();
    assert_eq!(dropped, [0, 1, 2]);
    assert_eq!(center.sent_at(), [0, 3, 8]);
}

#[tokio::test(start_paused = true)]
async fn a_window_whose_document_cant_be_written_is_dropped_and_counted_in_the_next() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock);
    let wall = clock.wall();
    let reads = Arc::new(AtomicUsize::new(0));
    // The first document's `generated_at` is past the year 9999, which it can't write.
    let now: WallClock = Arc::new(move || {
        if reads.fetch_add(1, Ordering::SeqCst) == 0 {
            UNIX_EPOCH + Duration::from_secs(400_000_000_000)
        } else {
            wall()
        }
    });
    let reporter =
        CallRecordReporter::start(&Handle::current(), center.clone(), now, Jitter::new(|| 1.0));

    reporter.report(window(8, &["/a"]));
    reporter.report(window(8, &["/b"]));
    until(&clock, 1).await;

    let dropped: Vec<_> = center.documents().iter().map(dropped).collect();
    assert_eq!(dropped, [1]);
}

#[tokio::test(start_paused = true)]
async fn sending_now_skips_the_backoff_even_for_a_send_in_flight() {
    // Whether the first send is still in flight when told; the sends that fail; when each came.
    let cases: [(bool, usize, &[u64]); 2] = [
        (false, 1, &[0, 1]),
        // The hurried failure still counts: the next one waits 10 s.
        (true, 2, &[0, 2, 12]),
    ];
    for (in_flight, failing, sent_at) in cases {
        let clock = TestClock::new();
        let center = Center::new(
            &clock,
            move |n| if n < failing { failed() } else { accepted() },
        )
        .holding();
        if !in_flight {
            center.release.add_permits(3);
        }
        let reporter = center.start(&clock);

        reporter.report(window(8, &["/a"]));
        until(&clock, 1).await;
        reporter.send_now();
        until(&clock, 2).await;
        center.release.add_permits(3);
        until(&clock, 13).await;

        assert_eq!(center.sent_at(), sent_at, "{in_flight}");
    }
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

    // It ends once each was tried.
    assert_eq!(center.sent_at(), [0, 1, 1, 1]);
    assert_eq!(clock.started.elapsed(), Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn the_flush_ends_whatever_is_in_flight_for_each_caller() {
    let clock = TestClock::new();
    let center = Center::accepting(&clock).holding();
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    reporter.report(window(8, &["/b"]));
    tokio::join!(reporter.flush(), reporter.flush());

    assert_eq!(center.sent_at(), [0]);
}

#[tokio::test]
async fn a_document_is_posted_to_rail_center_with_the_credential_and_read_by_its_status() {
    // A body the client would refuse to read, past its caps and in an encoding it doesn't know.
    let page = vec![b'x'; 128 * 1024];
    let reply = response("202 Accepted", &["content-encoding: br"], &page);
    let server = serve(move |_| Reply::Raw(reply.clone())).await;
    let sink = RailCenterCallRecords {
        client: client(&server.url),
    };
    let answer = sink.send(br#"{"window": "x"}"#.to_vec()).await;

    assert_posted(&server, "/v1/call-records", r#"{"window": "x"}"#);
    assert_eq!(answer.unwrap(), StatusCode::ACCEPTED);
}
