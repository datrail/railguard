use std::sync::Arc;
use std::time::Duration;

use rail_runtime::{Answer, Call, Gateway, GatewaySettings};

use crate::bundles::{self, Script};
use crate::holder::TestClock;
use crate::keys;
use crate::logs::{Logs, lines_with};
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
