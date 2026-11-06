use reqwest::StatusCode;

use crate::holder::{Clock, until};
use crate::keys::{Answer, Script, answer, failed, ok, start};
use crate::logs::Logs;

// The log of a key set holder answered with `answers(n)`, up to `seconds`.
async fn log_until(
    seconds: u64,
    answers: impl Fn(usize) -> Answer + Send + Sync + 'static,
) -> String {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = Clock::new();
    let script = Script::new(&clock, answers);

    let _holder = start(&clock, &script);
    until(&clock, seconds).await;

    logs.text()
}

fn lines_with<'a>(logs: &'a str, text: &str) -> Vec<&'a str> {
    logs.lines().filter(|line| line.contains(text)).collect()
}

#[tokio::test(start_paused = true)]
async fn the_set_is_logged_when_its_kids_change_and_only_then() {
    let logs = log_until(600, |n| if n < 2 { ok(&["b", "a"]) } else { ok(&["c"]) }).await;

    let lines = lines_with(&logs, "holding signing keys");
    assert_eq!(lines.len(), 2, "{logs}");
    assert!(lines[0].contains(r#"kids="a, b""#), "{logs}");
    assert!(lines[1].contains(r#"kids="c""#), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn a_failure_is_a_warning_while_a_set_is_held_and_an_error_once_none_is() {
    let warm = log_until(300, |n| if n == 0 { ok(&["a"]) } else { failed() }).await;
    let cold = log_until(0, |_| failed()).await;

    let warm = lines_with(&warm, "signing key set fetch failed; keeping the set held");
    assert_eq!(warm.len(), 1, "{warm:?}");
    assert!(
        warm[0].contains("WARN") && warm[0].contains(r#"kids="a""#),
        "{}",
        warm[0]
    );
    assert!(warm[0].contains("retry=5"), "{}", warm[0]);
    let cold = lines_with(
        &cold,
        "signing key set fetch failed; none is held, so every signed ticket reads bad_signature",
    );
    assert_eq!(cold.len(), 1, "{cold:?}");
    assert!(cold[0].contains("ERROR"), "{}", cold[0]);
}

#[tokio::test(start_paused = true)]
async fn a_set_past_its_age_is_logged_as_none_held_and_its_return_as_news() {
    // Held from 0, failing from 300 to 915, back at 1215.
    let logs = log_until(1215, |n| match n {
        0 | 9 => ok(&["a"]),
        _ => failed(),
    })
    .await;

    let failed_lines = lines_with(&logs, "signing key set fetch failed");
    assert!(
        failed_lines.last().unwrap().contains("none is held"),
        "{logs}"
    );
    assert_eq!(lines_with(&logs, "holding signing keys").len(), 2, "{logs}");
}

#[tokio::test(start_paused = true)]
async fn a_refused_credential_says_so() {
    let logs = log_until(0, |_| answer(StatusCode::UNAUTHORIZED, b"")).await;

    let line = lines_with(&logs, "signing key set fetch failed");
    assert_eq!(line.len(), 1, "{logs}");
    assert!(
        line[0].contains("Rail Center refused the credential (401 Unauthorized)")
            && line[0].contains("RAIL_AUTH_MODE and RAIL_AUTH_TOKEN"),
        "{logs}"
    );
}
