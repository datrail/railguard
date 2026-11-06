use rail_policy::TicketState;
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
