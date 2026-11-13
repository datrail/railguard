use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use rail_runtime::{Answer, Call, Gateway, GatewaySettings};

use crate::bundles::{self, Script};
use crate::holder::{TestClock, until};
use crate::keys;
use crate::logs::{Logs, lines_with};
use crate::scanning::{SCANNED, scanning, scanning_settings};
use crate::sink::Sink;

#[tokio::test(start_paused = true)]
async fn a_body_past_the_cap_is_warned_of_with_its_size_and_the_cap() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = TestClock::new();
    let never = Script {
        takes: Duration::from_secs(3600),
        ..Script::new(&clock, |_| bundles::failed())
    };
    let gateway = Gateway::new(
        Arc::new(bundles::start(&never, bundles::MINUTE)),
        Arc::new(keys::start(&clock, &Script::new(&clock, |_| keys::ok(&[])))),
        Arc::new(Sink::accepting().start()),
        clock.wall(),
        GatewaySettings {
            max_body_bytes: 4,
            ..GatewaySettings::default()
        },
    );
    let call = Call {
        path: "/mcp".into(),
        method: "POST".into(),
        body: "12345".to_owned(),
        headers: Vec::new(),
    };

    assert_eq!(gateway.handle(call).await.0, Answer::TooLarge);

    let logs = logs.text();
    let lines = lines_with(&logs, "past RAIL_MAX_BODY_MB");
    assert_eq!(lines.len(), 1, "{logs}");
    for field in ["WARN", "bytes=5", "limit=4"] {
        assert!(lines[0].contains(field), "{field}: {}", lines[0]);
    }
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
    let call = || Call {
        path: "/mcp".into(),
        method: "POST".into(),
        body: r#"{"method": "resources/read"}"#.to_owned(),
        headers: Vec::new(),
    };
    let scanned = Call {
        headers: vec![("x-a".into(), "a".repeat(SCANNED))],
        ..call()
    };

    // Left unpolled once it holds the slot, so its deadline can't stop its judgement.
    let mut judged = pin!(gateway.handle(scanned));
    let unfinished = tokio::time::timeout(Duration::from_millis(20), judged.as_mut()).await;
    assert!(unfinished.is_err());
    let (waiting, _) = gateway.handle(call()).await;
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
