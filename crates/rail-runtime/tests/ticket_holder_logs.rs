use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use rail_runtime::{
    Jitter, RailCenterTickets, TicketAnswer, TicketHolder, TicketQuery, TicketSettings,
};
use tokio::runtime::Handle;

use crate::common::{Reply, client, response, serve};
use crate::holder::{Clock, HOUR, Script, failed, start, ticket, token, until};
use crate::logs::Logs;

#[tokio::test(start_paused = true)]
async fn a_panicking_fetch_is_a_failed_attempt_logged_without_its_message() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = Clock::new();
    let expiry = clock.at(900);
    let script = Script::new(&clock, move |n| match n {
        0 => ticket("a", expiry),
        1 => panic!("a defect in the fetch"),
        _ => failed(),
    });

    let holder = start(&clock, &script, HOUR);
    until(&clock, 455).await;

    // Kept through the panic at 450, and the next attempt backs off as after any failure.
    assert_eq!(token(&holder), Ok("a".to_owned()));
    assert_eq!(script.fetched_at(), [0, 450, 455]);
    let logs = logs.text();
    assert!(
        logs.contains("a ticket refresh attempt panicked; keeping what's held"),
        "{logs}"
    );
    assert!(!logs.contains("a defect in the fetch"), "{logs}");
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
        settings: TicketSettings {
            query: TicketQuery {
                host_id: "host-1".to_owned(),
                sandbox_name: "sandbox a".to_owned(),
            },
            refresh_interval: HOUR,
        },
    };

    let _holder = TicketHolder::start(
        &Handle::current(),
        source,
        Arc::new(SystemTime::now),
        Jitter::random(),
    );
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

#[tokio::test(start_paused = true)]
async fn each_answer_is_logged_for_what_it_did_to_the_ticket_held() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = Clock::new();
    let (first, second, lapsed, last) =
        (clock.at(900), clock.at(1800), clock.at(0), clock.at(5000));
    let script = Script::new(&clock, move |n| match n {
        0 | 1 => ticket("a", first),
        2 => ticket("b", second),
        3 => failed(),
        4 => Ok(TicketAnswer::NoTicket),
        5 => failed(),
        6 => Ok(TicketAnswer::NoTicket),
        7 => ticket("c", lapsed),
        _ => ticket("d", last),
    });

    let _holder = start(&clock, &script, HOUR);
    until(&clock, 3600).await;

    let logs = logs.text();
    let expected = [
        format!("ticket acquired fingerprint=\"{}\"", fingerprint("a")),
        format!("ticket rotated fingerprint=\"{}\"", fingerprint("b")),
        format!(
            "ticket refresh failed; keeping the one held reason=\"the fetch ran past its 10s deadline\" \
             fingerprint=\"{}\"",
            fingerprint("b")
        ),
        "Rail Center holds no ticket for this host and sandbox cleared=true".to_owned(),
        "ticket refresh failed; none is held".to_owned(),
        "Rail Center holds no ticket for this host and sandbox cleared=false".to_owned(),
        "Rail Center returned a lapsed ticket; none is valid".to_owned(),
        // A lapsed ticket held is replaced, not acquired anew.
        format!("ticket rotated fingerprint=\"{}\"", fingerprint("d")),
    ];
    let mut lines = logs.lines();
    for line in &expected {
        assert!(lines.any(|logged| logged.contains(line)), "{line}\n{logs}");
    }
    // The same ticket again, a's and d's, is logged only at debug.
    assert_eq!(logs.matches("ticket rotated").count(), 2, "{logs}");
    assert_eq!(logs.matches("ticket acquired").count(), 1, "{logs}");
}
