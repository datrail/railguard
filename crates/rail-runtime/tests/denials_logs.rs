use rail_policy::{FallbackDenial, TicketState};
use rail_runtime::{Denial, DeniedBy, FetchError};
use reqwest::StatusCode;

use crate::logs::{Logs, lines_with};
use crate::sink::{Answering, POLICY, Sink, accepted, resolved};

// The log of one report answered with `answer`.
async fn log_of(answer: Answering) -> String {
    let logs = Logs::default();
    let _logging = logs.capture();
    let sink = Sink::answering(answer);
    let endpoint = resolved();

    sink.start().report(&Denial {
        denied_by: DeniedBy::Policy(POLICY),
        endpoint: &endpoint,
        ticket: &TicketState::Absent,
        claimed_status: None,
    });
    sink.until_sent(1).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }

    logs.text()
}

#[tokio::test]
async fn a_report_rail_center_accepts_isnt_warned_of() {
    let logs = log_of(|| accepted(StatusCode::ACCEPTED)).await;

    assert!(!logs.contains("WARN"), "{logs}");
}

#[tokio::test]
async fn a_report_not_accepted_is_warned_of_with_why_and_what_it_reported() {
    let answers: [(Answering, &str); 3] = [
        (|| accepted(StatusCode::OK), "Rail Center answered 200 OK"),
        (
            || accepted(StatusCode::UNAUTHORIZED),
            "Rail Center refused the credential",
        ),
        (
            || Err(FetchError::Transport("connection reset".to_owned())),
            "connection reset",
        ),
    ];
    for (answer, reason) in answers {
        let logs = log_of(answer).await;

        let lines = lines_with(&logs, "denial report failed");
        assert_eq!(lines.len(), 1, "{logs}");
        let line = lines[0];
        assert!(line.contains("WARN") && line.contains(reason), "{line}");
        assert!(line.contains(&format!(r#"policy="{POLICY}""#)), "{line}");
        assert!(
            line.contains(r#"endpoint_key="/mcp#tools/call#getPrice""#),
            "{line}"
        );
    }
}

#[tokio::test]
async fn a_dropped_report_is_warned_of_with_the_count() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let sink = Sink::holding();
    let reporter = sink.start();
    let endpoint = resolved();
    let denial = Denial {
        denied_by: DeniedBy::Policy(POLICY),
        endpoint: &endpoint,
        ticket: &TicketState::Absent,
        claimed_status: None,
    };

    for _ in 0..66 {
        reporter.report(&denial);
    }

    let logs = logs.text();
    let lines = lines_with(&logs, "dropping a denial report");
    assert_eq!(lines.len(), 2, "{logs}");
    assert!(
        lines[1].contains("WARN")
            && lines[1].contains("in_flight=64 ")
            && lines[1].contains("dropped=2"),
        "{}",
        lines[1]
    );
}

// The log of `count` reports of `denied_by` to a sink holding them in flight.
fn log_of_held(denied_by: DeniedBy, count: usize) -> String {
    let logs = Logs::default();
    let _logging = logs.capture();
    let sink = Sink::holding();
    let reporter = sink.start();
    let endpoint = resolved();
    let denial = Denial {
        denied_by,
        endpoint: &endpoint,
        ticket: &TicketState::Absent,
        claimed_status: None,
    };
    for _ in 0..count {
        reporter.report(&denial);
    }
    logs.text()
}

#[tokio::test]
async fn a_dropped_report_is_warned_of_with_what_it_reported_and_what_was_in_flight() {
    let fallback = log_of_held(DeniedBy::Fallback(FallbackDenial::Ticket), 17);
    let policy = log_of_held(DeniedBy::Policy(POLICY), 65);

    let fallback = lines_with(&fallback, "dropping a denial report");
    assert_eq!(fallback.len(), 1, "{fallback:?}");
    for field in [
        r#"fallback="ticket""#,
        r#"endpoint_key="/mcp#tools/call#getPrice""#,
        "in_flight=16 ",
        "naming_no_policy=16 ",
    ] {
        assert!(fallback[0].contains(field), "{field}: {}", fallback[0]);
    }
    let policy = lines_with(&policy, "dropping a denial report");
    assert_eq!(policy.len(), 1, "{policy:?}");
    for field in [
        format!(r#"policy="{POLICY}""#),
        "in_flight=64 ".to_owned(),
        "naming_no_policy=0 ".to_owned(),
    ] {
        assert!(policy[0].contains(&field), "{field}: {}", policy[0]);
    }
}

#[tokio::test]
async fn a_failed_report_naming_no_policy_is_warned_of_with_its_fallback() {
    for (fallback, name) in [
        (FallbackDenial::Ticket, "ticket"),
        (FallbackDenial::Binding, "binding"),
    ] {
        let logs = Logs::default();
        let _logging = logs.capture();
        let sink = Sink::answering(|| accepted(StatusCode::OK));
        let endpoint = resolved();

        sink.start().report(&Denial {
            denied_by: DeniedBy::Fallback(fallback),
            endpoint: &endpoint,
            ticket: &TicketState::Absent,
            claimed_status: None,
        });
        sink.until_sent(1).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        let logs = logs.text();
        let lines = lines_with(&logs, "denial report failed");
        assert_eq!(lines.len(), 1, "{logs}");
        assert!(
            lines[0].contains(&format!(r#"fallback="{name}""#)),
            "{}",
            lines[0]
        );
    }
}
