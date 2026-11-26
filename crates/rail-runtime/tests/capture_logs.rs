use rail_model::body::{BodyRead, Unread};
use rail_runtime::{Capture, CaptureThread};

use crate::call_record_center::Center;
use crate::capture_plans::{MINUTE, start_on};
use crate::captures::{a_plan, capture, post};
use crate::holder::{TestClock, until};
use crate::logs::{Logs, lines_with};
use crate::scripted::Script;

#[tokio::test(start_paused = true)]
async fn each_reason_a_body_went_unread_is_warned_of_once() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = TestClock::new();
    let center = Center::accepting(&clock);
    let (_capture, sender) = capture(&clock, |_| a_plan(), &center);

    until(&clock, 1).await;
    // The plan's entry takes POST /mcp on its body, which these weren't kept.
    for _ in 0..2 {
        sender.send(post("/mcp", BodyRead::Unread(Unread::PastCap)));
    }
    sender.send(post("/mcp", BodyRead::Unread(Unread::NotSent)));
    until(&clock, 2).await;

    let logs = logs.text();
    let lines = lines_with(&logs, "went to the catch-all");
    assert_eq!(lines.len(), 2, "{logs}");
    for (line, reason) in lines.iter().zip(["past_cap", "not_sent"]) {
        assert!(line.contains("WARN"), "{logs}");
        assert!(line.contains(&format!(r#"reason="{reason}""#)), "{logs}");
    }
}

#[tokio::test]
async fn a_stop_after_its_thread_stopped_is_warned_of() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = TestClock::new();
    let center = Center::accepting(&clock);
    let thread = CaptureThread::start().unwrap();
    let plans = start_on(thread.runtime(), &Script::new(&clock, |_| a_plan()), MINUTE);
    let (capture, _sender) = Capture::start(
        thread.runtime(),
        plans,
        center.start(&clock),
        clock.wall(),
        usize::MAX,
    );

    // Stopped the wrong way round: the task goes with its thread.
    thread.stop();
    capture.stop().await;

    let logs = logs.text();
    let lines = lines_with(&logs, "capture stopped before it could close its window");
    assert_eq!(lines.len(), 1, "{logs}");
    assert!(lines[0].contains("WARN"), "{logs}");
}
