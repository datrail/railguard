use rail_policy::FallbackDenial;
use rail_runtime::DeniedBy;
use reqwest::StatusCode;

use crate::logs::{Logs, lines_with};
use crate::sink::{POLICY, Sink, denial};

#[tokio::test]
async fn a_report_not_accepted_is_warned_of_with_why_and_what_it_reported() {
    for (answer, warned) in [(StatusCode::ACCEPTED, 0), (StatusCode::OK, 1)] {
        let logs = Logs::default();
        let _logging = logs.capture();
        let sink = Sink::answering(answer);

        sink.start().report(&denial(DeniedBy::Policy(POLICY)));
        sink.until_sent(1).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        let logs = logs.text();
        let lines = lines_with(&logs, "denial report failed");
        assert_eq!(lines.len(), warned, "{answer}: {logs}");
        for line in lines {
            for field in [
                "WARN",
                "Rail Center answered 200 OK",
                &format!(r#"denied_by="Policy({POLICY})""#),
                r#"endpoint_key="/mcp#tools/call#getPrice""#,
            ] {
                assert!(line.contains(field), "{field}: {line}");
            }
        }
    }
}

#[tokio::test]
async fn a_dropped_report_is_warned_of_with_what_it_reported_and_the_count() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let reporter = Sink::holding().start();

    for _ in 0..18 {
        reporter.report(&denial(DeniedBy::Fallback(FallbackDenial::Ticket)));
    }

    let logs = logs.text();
    let lines = lines_with(&logs, "dropping a denial report");
    assert_eq!(lines.len(), 2, "{logs}");
    for field in [
        "WARN",
        r#"denied_by="Fallback(Ticket)""#,
        r#"endpoint_key="/mcp#tools/call#getPrice""#,
        "dropped=2",
    ] {
        assert!(lines[1].contains(field), "{field}: {}", lines[1]);
    }
}
