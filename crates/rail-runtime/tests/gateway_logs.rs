use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use rail_runtime::{Answer, BodyEnding, Gateway, GatewaySettings};

use crate::bundles::{self, Script};
use crate::calls::{Call, call, handle, scanned_call};
use crate::holder::{TestClock, until};
use crate::keys;
use crate::logs::{Logs, lines_with};
use crate::scanning::{scanning, scanning_settings};
use crate::sink::Sink;

// A gateway that never holds a bundle: a call refused before the wait for one is all it answers.
fn holding_nothing(clock: &TestClock, max_body_bytes: usize) -> Gateway {
    let never = Script {
        takes: Duration::from_secs(3600),
        ..Script::new(clock, |_| bundles::failed())
    };
    Gateway::new(
        Arc::new(bundles::start(&never, bundles::MINUTE)),
        Arc::new(keys::start(clock, &Script::new(clock, |_| keys::ok(&[])))),
        Arc::new(Sink::accepting().start()),
        clock.wall(),
        GatewaySettings {
            max_body_bytes,
            ..GatewaySettings::default()
        },
    )
}

#[tokio::test(start_paused = true)]
async fn a_body_past_the_cap_is_warned_of_with_its_size_and_the_cap() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let gateway = holding_nothing(&TestClock::new(), 4);
    let past = Call {
        body: "12345".into(),
        ..call("POST", None)
    };

    assert_eq!(handle(&gateway, past).await.0, Answer::TooLarge);

    let logs = logs.text();
    let lines = lines_with(&logs, "past RAIL_MAX_BODY_MB");
    assert_eq!(lines.len(), 1, "{logs}");
    for field in ["WARN", "bytes=5", "limit=4"] {
        assert!(lines[0].contains(field), "{field}: {}", lines[0]);
    }
}

// Its bytes gone, nothing is left to send on: an error, not a warning.
#[tokio::test(start_paused = true)]
async fn a_body_dropped_past_the_memory_budget_is_an_error() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let gateway = holding_nothing(&TestClock::new(), 512 << 20);
    let mut gatherer = gateway.gather([(":method", "POST"), (":path", "/mcp")]);
    // Zeroed lazily, so it takes no memory until read.
    gatherer.adopt(vec![0; 257 << 20]);
    gatherer.adopt(b"0".to_vec());

    let (answer, _) = gateway.handle(gatherer, BodyEnding::Whole).await;
    assert_eq!(answer, Answer::CannotApply);

    let logs = logs.text();
    let lines = lines_with(&logs, "past the memory budget");
    assert_eq!(lines.len(), 1, "{logs}");
    assert!(lines[0].contains("ERROR"), "{}", lines[0]);
}

// On real time: a worker holds back paused time, so a deadline can't pass while it runs. Judged,
// the calls would be forwarded too, so only the log shows they went unjudged.
#[tokio::test]
async fn a_call_with_no_verdict_by_its_deadline_is_forwarded_at_observe() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = TestClock::new();
    let bundle = scanning("observe");
    let gateway = Gateway::new(
        Arc::new(bundles::start(
            &Script::new(&clock, move |_| bundles::ok(&bundle)),
            bundles::MINUTE,
        )),
        Arc::new(keys::start(&clock, &Script::new(&clock, |_| keys::ok(&[])))),
        Arc::new(Sink::accepting().start()),
        clock.wall(),
        scanning_settings(Duration::from_millis(100)),
    );
    until(&clock, 0).await;
    // Left unpolled once it holds the slot, so its deadline can't stop its judgement.
    let mut judged = pin!(handle(&gateway, scanned_call()));
    let unfinished = tokio::time::timeout(Duration::from_millis(20), judged.as_mut()).await;
    assert!(unfinished.is_err());
    let (waiting, _) = handle(&gateway, call("POST", None)).await;
    let (judged, _) = judged.await;

    assert_eq!(waiting, Answer::Forward);
    assert_eq!(judged, Answer::Forward);
    // In the order the calls were answered: the waiting one first.
    let logs = logs.text();
    let mut lines = logs
        .lines()
        .filter(|line| line.contains("the call went unjudged"));
    for why in [
        "its deadline passed waiting for a free slot",
        "its deadline passed while it was judged",
    ] {
        let line = lines.next().unwrap_or_default();
        assert!(line.contains(why), "{why}: {logs}");
        assert!(line.contains(r#"answer="Forward""#), "{line}");
    }
    assert_eq!(lines.next(), None, "{logs}");
}
