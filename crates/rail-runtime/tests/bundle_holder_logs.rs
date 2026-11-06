use std::sync::Mutex;
use std::time::Duration;

use rail_runtime::{BundleHolder, FetchError, Jitter, RailCenterBundles};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::runtime::Handle;

use crate::bundles::{Answer, MINUTE, Script, TWO, answer, bundle, failed, held_hash, ok, start};
use crate::common::{Reply, client, response, serve};
use crate::holder::{Clock, until};
use crate::logs::Logs;

// The log of a holder answered with `answers` in turn, once the last is fetched.
async fn log_of(answers: Vec<Answer>) -> String {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = Clock::new();
    let count = answers.len();
    let answers: Vec<_> = answers
        .into_iter()
        .map(|answer| Mutex::new(Some(answer)))
        .collect();
    let script = Script::new(&clock, move |n| answers[n].lock().unwrap().take().unwrap());

    let _holder = start(&script, MINUTE);
    while script.fetched_at().len() < count {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    // No attempt comes under 5 s after the last, so this only lets it be logged.
    tokio::time::sleep(Duration::from_secs(1)).await;

    logs.text()
}

fn lines_with<'a>(logs: &'a str, text: &str) -> Vec<&'a str> {
    logs.lines().filter(|line| line.contains(text)).collect()
}

fn enforcing(content_hash: &str, binding_fallback: &str, ticket_fallback: &str) -> Value {
    bundle(
        content_hash,
        json!({
            "enforcement": {"mode": "enforce"},
            "binding_fallback": binding_fallback,
            "ticket_fallback": ticket_fallback,
        }),
    )
}

#[tokio::test(start_paused = true)]
async fn the_holding_line_names_the_content_hash_and_the_schema_version() {
    let logs = log_of(vec![ok(&bundle(
        "hash-7f3a",
        json!({"schema_version": "2.4"}),
    ))])
    .await;

    let line = lines_with(&logs, "holding a policy bundle");
    assert_eq!(line.len(), 1, "{logs}");
    assert!(line[0].contains("INFO"), "{logs}");
    assert!(line[0].contains(r#"content_hash="hash-7f3a""#), "{logs}");
    assert!(line[0].contains(r#"schema_version="2.4""#), "{logs}");
    assert!(line[0].contains("enabled_policies=1 bindings=0"), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn the_posture_is_logged_when_a_poll_moves_it() {
    let logs = log_of(vec![
        ok(&bundle("v-none", json!({"enforcement": {"mode": "none"}}))),
        ok(&enforcing("v-enforce", "block", "pass")),
    ])
    .await;

    let lines = lines_with(&logs, "posture:");
    assert_eq!(lines.len(), 2, "{logs}");
    assert!(
        lines[0].contains("Rail Center says judge nothing"),
        "{logs}"
    );
    assert!(lines[0].contains(r#"enforcement="none""#), "{logs}");
    assert!(lines[1].contains(r#"enforcement="enforce""#), "{logs}");
    assert!(
        lines[1].contains(r#"binding_fallback="block" ticket_fallback="pass""#),
        "{logs}"
    );
}

#[tokio::test(start_paused = true)]
async fn observe_is_told_apart_in_the_log() {
    let logs = log_of(vec![ok(&bundle("v1", json!({})))]).await;

    let lines = lines_with(&logs, "posture:");
    assert_eq!(lines.len(), 1, "{logs}");
    assert!(lines[0].contains(r#"enforcement="observe""#), "{logs}");
    assert!(lines[0].contains("nothing is refused"), "{logs}");
    assert!(!lines[0].contains("fallback"), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn an_unchanged_posture_is_not_repeated_on_every_poll() {
    let logs = log_of(vec![
        ok(&enforcing("v1", "block", "pass")),
        ok(&enforcing("v2", "block", "pass")),
    ])
    .await;

    assert_eq!(lines_with(&logs, "posture:").len(), 1, "{logs}");
    assert_eq!(
        lines_with(&logs, "holding a policy bundle").len(),
        2,
        "{logs}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_fallback_that_moves_under_enforce_is_logged() {
    for (second, moved) in [
        (
            enforcing("v2", "pass", "pass"),
            r#"binding_fallback="pass""#,
        ),
        (
            enforcing("v2", "block", "block"),
            r#"ticket_fallback="block""#,
        ),
    ] {
        let logs = log_of(vec![ok(&enforcing("v1", "block", "pass")), ok(&second)]).await;

        let lines = lines_with(&logs, "posture:");
        assert_eq!(lines.len(), 2, "{logs}");
        assert!(lines[1].contains(moved), "{logs}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_fallback_that_moves_where_it_decides_nothing_is_not_logged() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = Clock::new();
    let script = Script::new(&clock, |n| {
        let fallback = if n == 0 { "block" } else { "pass" };
        ok(&bundle(
            &format!("v{n}"),
            json!({"binding_fallback": fallback}),
        ))
    });

    let holder = start(&script, MINUTE);
    until(&clock, 60).await;

    let logs = logs.text();
    assert_eq!(lines_with(&logs, "posture:").len(), 1, "{logs}");
    assert_eq!(
        lines_with(&logs, "holding a policy bundle").len(),
        2,
        "{logs}"
    );
    let held = holder.snapshot().unwrap();
    assert_eq!(held.posture.binding_fallback, rail_policy::Fallback::Pass);
}

#[tokio::test(start_paused = true)]
async fn a_fallback_held_silently_reports_itself_when_enforce_arrives() {
    let logs = log_of(vec![
        ok(&bundle("v1", json!({"binding_fallback": "pass"}))),
        ok(&enforcing("v2", "pass", "pass")),
    ])
    .await;

    let lines = lines_with(&logs, "posture:");
    assert_eq!(lines.len(), 2, "{logs}");
    assert!(lines[1].contains(r#"binding_fallback="pass""#), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn a_bundle_naming_no_posture_is_refused() {
    let mut body = bundle("old-rc", json!({}));
    body.as_object_mut().unwrap().remove("enforcement");

    let logs = log_of(vec![ok(&body)]).await;

    let line = lines_with(&logs, "refusing the fetched policy bundle; none is held");
    assert_eq!(line.len(), 1, "{logs}");
    assert!(line[0].contains("missing field `enforcement`"), "{logs}");
    assert!(lines_with(&logs, "posture:").is_empty(), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn another_major_is_refused_naming_both_versions() {
    let logs = log_of(vec![
        ok(&bundle("v1", json!({}))),
        ok(&bundle(
            "v2",
            json!({"schema_version": "3.0", "policies": "a new shape"}),
        )),
    ])
    .await;

    let line = lines_with(
        &logs,
        "refusing the fetched policy bundle; keeping the one held",
    );
    assert_eq!(line.len(), 1, "{logs}");
    assert!(line[0].contains("schema_version 3.0"), "{logs}");
    assert!(line[0].contains("it reads 2.x"), "{logs}");
    assert!(line[0].contains(r#"content_hash="v1""#), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn a_bundle_read_as_unchanged_is_not_refused() {
    let logs = log_of(vec![
        ok(&bundle("v1", json!({}))),
        ok(&bundle("v1", json!({"policies": "not a list"}))),
    ])
    .await;

    assert!(lines_with(&logs, "refusing").is_empty(), "{logs}");
    assert_eq!(
        lines_with(&logs, "holding a policy bundle").len(),
        1,
        "{logs}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_root_field_this_reader_does_not_know_is_left_unread() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = Clock::new();
    let rejected = json!({"rejected": [{"policy_id": TWO, "reason": "condition not evaluable"}]});
    let script = Script::new(&clock, move |_| ok(&bundle("v1", rejected.clone())));

    let holder = start(&script, MINUTE);
    until(&clock, 0).await;

    assert_eq!(held_hash(&holder).as_deref(), Some("v1"));
    let logs = logs.text();
    for unread in ["rejected", TWO, "condition not evaluable"] {
        assert!(!logs.contains(unread), "{logs}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_failure_is_a_warning_while_a_bundle_is_held_and_an_error_once_none_is() {
    let unavailable = || answer(StatusCode::SERVICE_UNAVAILABLE, b"");
    let cold = log_of(vec![unavailable()]).await;
    let warm = log_of(vec![ok(&bundle("v1", json!({}))), unavailable()]).await;

    let cold = lines_with(&cold, "policy bundle fetch failed; none is held");
    assert_eq!(cold.len(), 1);
    assert!(cold[0].contains("ERROR"), "{}", cold[0]);
    assert!(cold[0].contains(r#"reason="Rail Center answered 503 Service Unavailable""#));
    let warm = lines_with(&warm, "policy bundle fetch failed; keeping the one held");
    assert_eq!(warm.len(), 1);
    assert!(warm[0].contains("WARN"), "{}", warm[0]);
}

#[tokio::test(start_paused = true)]
async fn a_refused_bundle_is_an_error_whether_or_not_one_is_held() {
    let refused = || ok(&bundle("v2", json!({"policies": "not a list"})));
    let cold = log_of(vec![refused()]).await;
    let warm = log_of(vec![ok(&bundle("v1", json!({}))), refused()]).await;

    for (logs, line) in [
        (cold, "refusing the fetched policy bundle; none is held"),
        (
            warm,
            "refusing the fetched policy bundle; keeping the one held",
        ),
    ] {
        let lines = lines_with(&logs, line);
        assert_eq!(lines.len(), 1, "{logs}");
        assert!(lines[0].contains("ERROR"), "{logs}");
        assert!(lines[0].contains("unusable policy bundle"), "{logs}");
    }
}

#[tokio::test(start_paused = true)]
async fn holding_nothing_says_nothing_of_what_traffic_meets() {
    for first in [
        answer(StatusCode::SERVICE_UNAVAILABLE, b""),
        ok(&bundle("v1", json!({"policies": "not a list"}))),
    ] {
        let logs = log_of(vec![first]).await;

        let lines = lines_with(&logs, "none is held");
        assert_eq!(lines.len(), 1, "{logs}");
        for word in ["traffic", "forward", "enforced", "request"] {
            assert!(!lines[0].contains(word), "{logs}");
        }
    }
}

#[tokio::test(start_paused = true)]
async fn both_failure_lines_name_the_held_bundle_alike() {
    let refused = log_of(vec![
        ok(&bundle("v-held", json!({}))),
        ok(&bundle("v-next", json!({"policies": "not a list"}))),
    ])
    .await;
    let unreachable = log_of(vec![ok(&bundle("v-held", json!({}))), failed()]).await;

    for (logs, line) in [
        (
            refused,
            "refusing the fetched policy bundle; keeping the one held",
        ),
        (
            unreachable,
            "policy bundle fetch failed; keeping the one held",
        ),
    ] {
        let lines = lines_with(&logs, line);
        assert_eq!(lines.len(), 1, "{logs}");
        assert!(lines[0].contains(r#"content_hash="v-held""#), "{logs}");
        assert!(!lines[0].contains("version"), "{logs}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_refused_credential_says_so_distinctly() {
    for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
        let logs = log_of(vec![answer(status, b"")]).await;

        let line = lines_with(&logs, "policy bundle fetch failed");
        assert_eq!(line.len(), 1, "{logs}");
        assert!(
            line[0].contains(&format!("Rail Center refused the credential ({status})")),
            "{logs}"
        );
        assert!(
            line[0].contains("RAIL_AUTH_MODE and RAIL_AUTH_TOKEN"),
            "{logs}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_body_that_is_not_json_is_a_failed_fetch_not_a_refused_bundle() {
    let logs = log_of(vec![answer(StatusCode::OK, b"<html>nope</html>")]).await;

    let line = lines_with(&logs, "policy bundle fetch failed; none is held");
    assert_eq!(line.len(), 1, "{logs}");
    assert!(line[0].contains("the response isn't JSON (Syntax error at line 1, column 1)"));
}

#[tokio::test(start_paused = true)]
async fn a_reason_never_carries_a_character_a_log_line_cannot_hold() {
    let hostile = "illegal header line: b'X-Forged: real\n2026-08-29 ERROR nothing\x1b[31m'";
    let logs = log_of(vec![Err(FetchError::Transport(hostile.to_owned()))]).await;

    let line = lines_with(&logs, "policy bundle fetch failed");
    assert_eq!(line.len(), 1, "{logs}");
    assert!(
        line[0].contains(r"X-Forged: real\u{a}2026-08-29 ERROR nothing\u{1b}[31m"),
        "{logs}"
    );
    assert_eq!(lines_with(&logs, "ERROR nothing").len(), 1, "{logs}");
}

#[tokio::test(start_paused = true)]
async fn a_panicking_fetch_is_a_failed_attempt_logged_without_its_message() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = Clock::new();
    let script = Script::new(&clock, |n| {
        if n == 0 {
            panic!("a defect in the fetch")
        } else {
            ok(&bundle("v1", json!({})))
        }
    });

    let holder = start(&script, MINUTE);
    until(&clock, 5).await;

    // Retried as after any failure.
    assert_eq!(held_hash(&holder).as_deref(), Some("v1"));
    assert_eq!(script.fetched_at(), [0, 5]);
    let logs = logs.text();
    let line = lines_with(
        &logs,
        "a policy bundle refresh attempt panicked; keeping what's held",
    );
    assert_eq!(line.len(), 1, "{logs}");
    assert!(!logs.contains("a defect in the fetch"), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn a_huge_refresh_interval_neither_panics_nor_stops_the_holder() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = Clock::new();
    let script = Script::new(&clock, |_| ok(&bundle("v1", json!({}))));

    let holder = start(&script, Duration::MAX);
    until(&clock, 3600).await;

    assert_eq!(held_hash(&holder).as_deref(), Some("v1"));
    assert_eq!(script.fetched_at(), [0]);
    let logs = logs.text();
    assert!(!logs.contains("panicked"), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn a_repeated_refusal_is_logged_at_each_attempt_with_when_the_next_comes() {
    let refused = || answer(StatusCode::UNAUTHORIZED, b"");
    let logs = log_of((0..5).map(|_| refused()).collect()).await;

    // At 0, 5, 15, 35 and 75: the backoff spaces the lines out.
    let lines = lines_with(&logs, "Rail Center refused the credential");
    assert_eq!(lines.len(), 5, "{logs}");
    for (line, retry) in lines.iter().zip([5, 10, 20, 40, 60]) {
        assert!(line.contains(&format!("retry={retry}")), "{line}");
    }
}

#[tokio::test(start_paused = true)]
async fn the_recovery_from_a_refusal_is_logged() {
    let refused = || answer(StatusCode::UNAUTHORIZED, b"");
    let logs = log_of(vec![refused(), refused(), ok(&bundle("v1", json!({})))]).await;

    assert_eq!(
        lines_with(&logs, "Rail Center refused the credential").len(),
        2,
        "{logs}"
    );
    assert_eq!(
        lines_with(&logs, "holding a policy bundle").len(),
        1,
        "{logs}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_bundle_fetched_again_after_a_failure_says_so_once() {
    let v1 = || ok(&bundle("v1", json!({})));
    let logs = log_of(vec![v1(), failed(), v1(), v1()]).await;
    assert_eq!(
        lines_with(&logs, "policy bundle fetched again, unchanged").len(),
        1,
        "{logs}"
    );

    let logs = log_of(vec![v1(), v1(), v1()]).await;
    assert!(!logs.contains("fetched again"), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn an_unchanged_bundle_after_a_panicking_attempt_says_it_was_fetched_again() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = Clock::new();
    let script = Script::new(&clock, |n| {
        if n == 1 {
            panic!("a defect in the fetch")
        } else {
            ok(&bundle("v1", json!({})))
        }
    });

    let _holder = start(&script, MINUTE);
    until(&clock, 65).await;

    let logs = logs.text();
    assert_eq!(
        lines_with(&logs, "policy bundle fetched again, unchanged").len(),
        1,
        "{logs}"
    );
}

#[tokio::test(start_paused = true)]
async fn the_retry_logged_is_the_jittered_wait_rounded_to_seconds() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = Clock::new();
    let script = Script::new(&clock, |_| answer(StatusCode::SERVICE_UNAVAILABLE, b""));

    // 5 s at 1.18 is 5.9 s.
    let _holder = BundleHolder::start(&Handle::current(), script, MINUTE, Jitter::new(|| 1.18));
    until(&clock, 0).await;

    let logs = logs.text();
    let line = lines_with(&logs, "policy bundle fetch failed");
    assert_eq!(line.len(), 1, "{logs}");
    assert!(line[0].contains("retry=6"), "{logs}");
}

// Real time: the fetch goes over a socket, which paused time would race.
#[tokio::test]
async fn a_bundle_of_exactly_4_mib_is_read_and_one_past_it_refused() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let cap = 4 * 1024 * 1024;
    for (bytes, held) in [(cap, true), (cap + 1, false)] {
        let mut body = bundle("v1", json!({})).to_string().into_bytes();
        body.resize(bytes, b' ');
        let server = serve(move |_| Reply::Raw(response("200 OK", &[], &body))).await;
        let source = RailCenterBundles {
            client: client(&server.url),
            gateway_slug: "edge".to_owned(),
        };

        let holder = BundleHolder::start(&Handle::current(), source, MINUTE, Jitter::new(|| 1.0));
        // Past the first attempt, and short of the retry 5 s after a failure.
        let waited = holder.wait_for_bundle(Duration::from_secs(4)).await;

        assert_eq!(waited.is_some(), held, "{bytes}");
    }
    let logs = logs.text();
    assert!(
        logs.contains(r#"reason="the response is past the 4194304 bytes read""#),
        "{logs}"
    );
}
