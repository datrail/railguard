use std::sync::Mutex;
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::bundles::{Answer, MINUTE, Script, TWO, answer, bundle, failed, ok, start};
use crate::holder::Clock;
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
async fn the_holding_line_names_the_content_hash_the_schema_version_and_what_it_holds() {
    let disabled = json!({
        "id": TWO,
        "name": "Q",
        "priority": 2,
        "condition": {"path": "ticket.posture.overall.score", "op": "lt", "value": 40},
        "action": "block",
        "enabled": false,
    });
    let binding = |tool| {
        let endpoint_key = format!("edge#/mcp#tools/call#{tool}");
        json!({"endpoint_key": endpoint_key, "mode": "open", "policy_ids": []})
    };
    let bindings = [binding("refund_order"), binding("cancel_order")];
    let mut held = bundle(
        "hash-7f3a",
        json!({"schema_version": "2.4", "bindings": bindings}),
    );
    held["policies"].as_array_mut().unwrap().push(disabled);
    let logs = log_of(vec![ok(&held)]).await;

    let line = lines_with(&logs, "holding a policy bundle");
    assert_eq!(line.len(), 1, "{logs}");
    assert!(line[0].contains("INFO"), "{logs}");
    assert!(line[0].contains(r#"content_hash="hash-7f3a""#), "{logs}");
    assert!(line[0].contains(r#"schema_version="2.4""#), "{logs}");
    assert!(line[0].contains("enabled_policies=1 bindings=2"), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn the_posture_is_logged_whenever_what_traffic_meets_moves() {
    let none = bundle("v-none", json!({"enforcement": {"mode": "none"}}));
    let observing = |content_hash, binding_fallback| {
        bundle(content_hash, json!({"binding_fallback": binding_fallback}))
    };
    // Each posture line expected: the mode, and the fallbacks it names.
    for (label, bundles, postures) in [
        (
            "none, then enforce",
            vec![none.clone(), enforcing("v2", "block", "pass")],
            vec![("none", None), ("enforce", Some(("block", "pass")))],
        ),
        (
            "none, then observe",
            vec![none, observing("v2", "block")],
            vec![("none", None), ("observe", None)],
        ),
        (
            "enforce, unchanged",
            vec![
                enforcing("v1", "block", "pass"),
                enforcing("v2", "block", "pass"),
            ],
            vec![("enforce", Some(("block", "pass")))],
        ),
        (
            "the binding fallback moving under enforce",
            vec![
                enforcing("v1", "block", "pass"),
                enforcing("v2", "pass", "pass"),
            ],
            vec![
                ("enforce", Some(("block", "pass"))),
                ("enforce", Some(("pass", "pass"))),
            ],
        ),
        (
            "the ticket fallback moving under enforce",
            vec![
                enforcing("v1", "block", "pass"),
                enforcing("v2", "block", "block"),
            ],
            vec![
                ("enforce", Some(("block", "pass"))),
                ("enforce", Some(("block", "block"))),
            ],
        ),
        (
            "a fallback moving where it decides nothing",
            vec![observing("v1", "block"), observing("v2", "pass")],
            vec![("observe", None)],
        ),
        (
            "a fallback held silently, then enforce",
            vec![observing("v1", "pass"), enforcing("v2", "pass", "pass")],
            vec![("observe", None), ("enforce", Some(("pass", "pass")))],
        ),
    ] {
        let count = bundles.len();
        let logs = log_of(bundles.iter().map(ok).collect()).await;

        let held = lines_with(&logs, "holding a policy bundle");
        assert_eq!(held.len(), count, "{label}: {logs}");
        let lines = lines_with(&logs, "posture:");
        assert_eq!(lines.len(), postures.len(), "{label}: {logs}");
        for (line, (mode, fallbacks)) in lines.iter().zip(postures) {
            let says = match mode {
                "none" => "Rail Center says judge nothing",
                "observe" => "nothing is refused",
                _ => "one denied is refused",
            };
            let fields = match fallbacks {
                None => format!(r#"enforcement="{mode}""#),
                Some((binding, ticket)) => format!(
                    r#"enforcement="{mode}" binding_fallback="{binding}" ticket_fallback="{ticket}""#
                ),
            };
            assert!(
                line.contains(says) && line.ends_with(&fields),
                "{label}: {line}"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn each_failure_is_logged_for_whom_it_blames_and_what_is_held() {
    let held = || ok(&bundle("v-held", json!({})));
    let refused = || ok(&bundle("v-next", json!({"policies": "not a list"})));
    let unavailable = || answer(StatusCode::SERVICE_UNAVAILABLE, b"");
    let fetch_failed = "policy bundle fetch failed; ";
    let refusing = "refusing the fetched policy bundle; ";
    let (none_held, keeping) = ("none is held", "keeping the one held");
    for (answers, line, level, says) in [
        (
            vec![unavailable()],
            [fetch_failed, none_held],
            "ERROR",
            vec![r#"reason="Rail Center answered 503 Service Unavailable""#],
        ),
        (
            vec![held(), failed()],
            [fetch_failed, keeping],
            "WARN",
            vec![
                r#"reason="the fetch ran past its 30s deadline""#,
                r#"content_hash="v-held""#,
            ],
        ),
        (
            vec![refused()],
            [refusing, none_held],
            "ERROR",
            vec!["unusable policy bundle"],
        ),
        (
            vec![held(), refused()],
            [refusing, keeping],
            "ERROR",
            vec!["unusable policy bundle", r#"content_hash="v-held""#],
        ),
        (
            vec![
                held(),
                ok(&bundle(
                    "v2",
                    json!({"schema_version": "3.0", "policies": "a new shape"}),
                )),
            ],
            [refusing, keeping],
            "ERROR",
            vec!["schema_version 3.0 ", "it reads 2.x"],
        ),
        (
            vec![answer(StatusCode::UNAUTHORIZED, b"")],
            [fetch_failed, none_held],
            "ERROR",
            vec![
                "Rail Center refused the credential (401 Unauthorized)",
                "RAIL_AUTH_MODE and RAIL_AUTH_TOKEN",
            ],
        ),
        (
            vec![answer(StatusCode::FORBIDDEN, b"")],
            [fetch_failed, none_held],
            "ERROR",
            vec![
                "Rail Center refused the credential (403 Forbidden)",
                "RAIL_AUTH_MODE and RAIL_AUTH_TOKEN",
            ],
        ),
    ] {
        let line = line.concat();
        let logs = log_of(answers).await;

        let lines = lines_with(&logs, &line);
        assert_eq!(lines.len(), 1, "{line}: {logs}");
        assert!(lines[0].contains(level), "{logs}");
        // The first failure since a success: the retry is 5 s away.
        assert!(lines[0].ends_with(" retry=5"), "{logs}");
        for text in says {
            assert!(lines[0].contains(text), "{logs}");
        }
        // The held bundle is named by its hash alone.
        assert!(!lines[0].contains("version="), "{logs}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_bundle_fetched_again_after_a_failure_says_so_once() {
    let v1 = || ok(&bundle("v1", json!({})));
    let logs = log_of(vec![v1(), failed(), v1(), v1()]).await;
    let line = lines_with(&logs, "policy bundle fetched again, unchanged");
    assert_eq!(line.len(), 1, "{logs}");
    assert!(line[0].contains(r#"content_hash="v1""#), "{logs}");

    let logs = log_of(vec![v1(), v1(), v1()]).await;
    assert!(!logs.contains("fetched again"), "{logs}");
}
