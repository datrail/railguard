use reqwest::StatusCode;
use serde_json::json;

use crate::capture_plans::{Answer, MINUTE, answer, hash, no_plan, ok, plan, start};
use crate::logs::{self, lines_with};

async fn log_of(answers: Vec<Answer>) -> String {
    logs::log_of(answers, |script| start(script, MINUTE)).await
}

#[tokio::test(start_paused = true)]
async fn a_plan_applied_is_said_with_its_hash_version_window_and_refused_entries() {
    let a = hash('a');
    let mut applied = plan(&a, json!({}));
    // A second entry, refused for an op the proxy doesn't know.
    let mut refused = applied["entries"][0].clone();
    refused["id"] = json!("e2");
    refused["match"][0]["op"] = json!("resembles");
    applied["entries"].as_array_mut().unwrap().push(refused);
    let logs = log_of(vec![ok(&applied)]).await;

    let line = lines_with(&logs, "holding a capture plan");
    assert_eq!(line.len(), 1, "{logs}");
    assert!(line[0].contains("INFO"), "{logs}");
    for field in [
        &format!(r#"content_hash="{a}""#),
        "plan_version=7",
        "window_seconds=900",
        "entries=2",
        "refused_entries=1",
        "entries_skipped=1",
    ] {
        assert!(line[0].contains(field), "{field}: {logs}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_plan_refused_is_warned_of_with_why_and_what_is_kept() {
    let (a, b) = (hash('a'), hash('b'));
    for (label, refused, said) in [
        (
            "another major",
            ok(&json!({"schema_version": "2.0", "content_hash": b})),
            "schema_version 2.0 is a plan shape this proxy doesn't read; it reads 1.x",
        ),
        (
            "a plan refused",
            ok(&plan(&b, json!({"window_seconds": 10}))),
            "window_seconds",
        ),
        (
            "another redaction_version",
            ok(&plan(&b, json!({"redaction_version": 2}))),
            "redaction_version 2: ",
        ),
        (
            // Compared last, so a plan refused for its shape is told by that.
            "a plan refused for its shape and its redaction_version",
            ok(&plan(
                &b,
                json!({"window_seconds": 10, "redaction_version": 2}),
            )),
            "window_seconds",
        ),
    ] {
        let logs = log_of(vec![refused]).await;
        let line = lines_with(&logs, "refusing the fetched capture plan; none is held");
        assert_eq!(line.len(), 1, "{label}: {logs}");
        assert!(line[0].contains("WARN"), "{label}: {logs}");
        assert!(
            line[0].contains(&format!(r#"refused_hash="{b}""#)),
            "{label}: {logs}"
        );
        assert!(line[0].contains(said), "{label}: {logs}");
        assert!(
            label == "another redaction_version" || !line[0].contains("redaction_version 2"),
            "{label}: {logs}"
        );
    }

    let logs = log_of(vec![
        ok(&plan(&a, json!({}))),
        ok(&plan(&b, json!({"redaction_version": 2}))),
    ])
    .await;
    let line = lines_with(
        &logs,
        "refusing the fetched capture plan; keeping the one held",
    );
    assert_eq!(line.len(), 1, "{logs}");
    assert!(
        line[0].contains(&format!(r#"content_hash="{a}""#)),
        "{logs}"
    );
    assert!(
        line[0].contains(&format!(r#"refused_hash="{b}""#)),
        "{logs}"
    );
}

#[tokio::test(start_paused = true)]
async fn the_same_word_again_is_said_at_info_only_after_a_failure() {
    let a = hash('a');
    let same = || ok(&plan(&a, json!({})));
    let failed = || answer(StatusCode::SERVICE_UNAVAILABLE, b"");
    // The first no plan is said too; the same plan's first is the line it's applied by.
    for (label, answers, said, times) in [
        (
            "no plan",
            vec![no_plan(), no_plan(), no_plan(), failed(), no_plan()],
            "Rail Center holds no capture plan for this agent",
            2,
        ),
        (
            "the same plan",
            vec![same(), same(), failed(), same()],
            "capture plan fetched again, unchanged",
            1,
        ),
    ] {
        let logs = log_of(answers).await;

        let lines = lines_with(&logs, said);
        assert_eq!(lines.len(), times, "{label}: {logs}");
        assert!(
            lines.iter().all(|line| line.contains("INFO")),
            "{label}: {logs}"
        );
        // A failure names the plan held, if any.
        let failed = lines_with(&logs, "capture plan fetch failed; ");
        let hash = format!(r#"content_hash="{a}""#);
        assert_eq!(
            failed[0].contains(&hash),
            label == "the same plan",
            "{label}: {logs}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn an_answer_that_is_no_plan_is_a_failure_saying_why() {
    // An array would fill the peek's fields in order, a hash and all.
    let array = json!([hash('a'), null, null]);
    for (answer, said) in [(ok(&array), "isn't an object")] {
        let logs = log_of(vec![answer]).await;

        let line = lines_with(&logs, "capture plan fetch failed; none is held");
        assert_eq!(line.len(), 1, "{logs}");
        assert!(line[0].contains("WARN") && line[0].contains(said), "{logs}");
    }
}

#[tokio::test(start_paused = true)]
async fn the_plan_held_cleared_or_named_again_is_said_as_such() {
    let a = hash('a');
    let other = json!({"agent": {"host_id": "host-7f3c", "sandbox_name": "other"}});

    let logs = log_of(vec![ok(&plan(&a, json!({}))), no_plan()]).await;
    let cleared = lines_with(&logs, "Rail Center holds no capture plan for this agent");
    assert!(cleared.len() == 1 && cleared[0].contains("INFO"), "{logs}");

    // The hash covers the agent too: the same one is the plan held, not another agent's.
    let logs = log_of(vec![ok(&plan(&a, json!({}))), ok(&plan(&a, other))]).await;
    assert!(!logs.contains("capture plan fetch failed"), "{logs}");
}
