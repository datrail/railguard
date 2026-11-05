use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use rail_runtime::{RailCenterTickets, TicketHolder, TicketQuery, TicketSettings, Unavailable};
use tokio::runtime::Handle;

use crate::common::{Reply, client, response, serve};
use crate::holder::{Clock, HOUR, Script, start, ticket, token, until};
use crate::logs::Logs;

#[tokio::test(start_paused = true)]
async fn a_panicking_refresh_loop_is_logged() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = Clock::new();
    let script = Script::new(&clock, |_| panic!("a defect in the fetch"));

    let holder = start(&clock, &script, HOUR);
    until(&clock, 1).await;

    assert_eq!(token(&holder), Err(Unavailable::IssuerUnreachable));
    let logs = logs.text();
    assert!(
        logs.contains("the ticket refresh loop panicked; the ticket is no longer refreshed"),
        "{logs}"
    );
}

fn fingerprint(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_lapsed_answer_kept_out_is_logged_by_its_own_fingerprint() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = Clock::new();
    let (good, lapsed) = (clock.at(900), clock.at(0));
    let script = Script::new(&clock, move |n| {
        if n == 0 {
            ticket("a", good)
        } else {
            ticket("b", lapsed)
        }
    });

    let _holder = start(&clock, &script, HOUR);
    until(&clock, 451).await;

    let logs = logs.text();
    let line = logs
        .lines()
        .find(|line| line.contains("returned a lapsed ticket; keeping the one held"))
        .unwrap_or_else(|| panic!("{logs}"));
    assert!(
        line.contains(&format!("lapsed=\"{}\"", fingerprint("b"))),
        "{line}"
    );
    assert!(
        line.contains(&format!("fingerprint=\"{}\"", fingerprint("a"))),
        "{line}"
    );
}

// Real time: the fetch goes over a socket, which paused time would race.
#[tokio::test]
async fn a_hostile_answer_is_logged_escaped_once() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let body = r#"{"host_id":"host-1\nfake","tickets":[]}"#;
    let server = serve(move |_| Reply::Raw(response("200 OK", &[], body.as_bytes()))).await;
    let source = RailCenterTickets {
        client: client(&server.url),
        settings: TicketSettings::new(TicketQuery {
            host_id: "host-1".to_owned(),
            sandbox_name: "sandbox a".to_owned(),
        }),
    };

    let _holder = TicketHolder::start(&Handle::current(), source, Arc::new(SystemTime::now));
    let deadline = Instant::now() + Duration::from_secs(10);
    let line = loop {
        let text = logs.text();
        if let Some(line) = text
            .lines()
            .find(|line| line.contains("ticket refresh failed"))
        {
            break line.to_owned();
        }
        assert!(Instant::now() < deadline, "{text}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    assert!(
        line.contains(r#"reason="the response answers for host host-1\u{a}fake, not host-1""#),
        "{line}"
    );
}
