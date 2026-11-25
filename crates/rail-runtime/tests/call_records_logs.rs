use reqwest::StatusCode;

use crate::call_record_center::{Center, accepted, window};
use crate::holder::{TestClock, until};
use crate::logs::{Logs, lines_with};
use crate::scripted::{answer, failed};

#[tokio::test(start_paused = true)]
async fn a_failed_send_is_warned_of_with_why_and_when_it_is_retried() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = TestClock::new();
    let center = Center::new(&clock, |n| if n == 0 { failed() } else { accepted() });
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    until(&clock, 6).await;

    let logs = logs.text();
    let lines = lines_with(
        &logs,
        "call records send failed; keeping them to send again",
    );
    assert_eq!(lines.len(), 1, "{logs}");
    for field in [
        "WARN",
        "the fetch ran past its 10s deadline",
        "document_id=",
        "queued=1",
        "retry=5",
    ] {
        assert!(lines[0].contains(field), "{field}: {logs}");
    }
    assert!(lines_with(&logs, "dropping").is_empty(), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn a_refused_document_is_an_error_and_its_drop_warned_of() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = TestClock::new();
    let center = Center::new(&clock, |_| answer(StatusCode::UNPROCESSABLE_ENTITY, b""));
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    until(&clock, 1).await;

    let logs = logs.text();
    let refused = lines_with(&logs, "Rail Center refused call records");
    assert_eq!(refused.len(), 1, "{logs}");
    assert!(refused[0].contains("ERROR"), "{logs}");
    assert!(refused[0].contains("Rail Center answered 422"), "{logs}");
    let dropped = lines_with(&logs, "dropping a call record window");
    assert_eq!(dropped.len(), 1, "{logs}");
    for field in [
        "WARN",
        r#"why="Rail Center refused it""#,
        "windows_dropped=1",
    ] {
        assert!(dropped[0].contains(field), "{field}: {logs}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_window_dropped_past_the_limit_is_warned_of() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = TestClock::new();
    let center = Center::accepting(&clock).holding();
    let reporter = center.start(&clock);

    reporter.report(window(0, &["/a"]));
    until(&clock, 1).await;
    reporter.report(window(0, &[]));
    until(&clock, 2).await;

    let logs = logs.text();
    let dropped = lines_with(&logs, "dropping a call record window");
    assert_eq!(dropped.len(), 1, "{logs}");
    for field in [
        "WARN",
        r#"why="the queue is past its limits""#,
        "empty=true",
        "windows_dropped=1",
    ] {
        assert!(dropped[0].contains(field), "{field}: {logs}");
    }
}

#[tokio::test(start_paused = true)]
async fn windows_left_when_the_flush_ends_are_warned_of() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = TestClock::new();
    let center = Center::accepting(&clock).holding();
    let reporter = center.start(&clock);

    reporter.report(window(8, &["/a"]));
    reporter.report(window(8, &["/b"]));
    reporter.flush().await;

    let logs = logs.text();
    let lines = lines_with(&logs, "call record windows left unsent at shutdown");
    assert_eq!(lines.len(), 1, "{logs}");
    assert!(lines[0].contains("WARN"), "{logs}");
    assert!(lines[0].contains("unsent=2"), "{logs}");
}
