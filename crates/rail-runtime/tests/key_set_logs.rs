use reqwest::StatusCode;
use serde_json::json;

use crate::holder::{TestClock, until};
use crate::keys::{Answer, Script, answer, failed, key_set, ok, start};
use crate::logs::{Logs, lines_with};

// The log of a key set holder answered with `answers(n)`, up to `seconds`.
async fn log_until(
    seconds: u64,
    answers: impl Fn(usize) -> Answer + Send + Sync + 'static,
) -> String {
    let logs = Logs::default();
    let _logging = logs.capture();
    let clock = TestClock::new();
    let script = Script::new(&clock, answers);

    let _holder = start(&clock, &script);
    until(&clock, seconds).await;

    logs.text()
}

#[tokio::test(start_paused = true)]
async fn the_set_is_logged_when_its_kids_change_and_only_then() {
    // The same kids listed in another order, then one swapped, then one more.
    let sets: [&[&str]; 4] = [&["b", "a"], &["a", "b"], &["a", "c"], &["a", "c", "d"]];
    let logs = log_until(900, move |n| ok(sets[n])).await;

    let lines = lines_with(&logs, "holding signing keys");
    let logged: Vec<&str> = ["a, b", "a, c", "a, c, d"].to_vec();
    assert_eq!(lines.len(), logged.len(), "{logs}");
    for (line, kids) in lines.iter().zip(logged) {
        assert!(line.contains(&format!("kids=\"{kids}\"")), "{line}");
    }
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
    for (status, shown) in [
        (StatusCode::UNAUTHORIZED, "401 Unauthorized"),
        (StatusCode::FORBIDDEN, "403 Forbidden"),
    ] {
        let logs = log_until(0, move |_| answer(status, b"")).await;

        let line = lines_with(&logs, "signing key set fetch failed");
        assert_eq!(line.len(), 1, "{logs}");
        assert!(
            line[0].contains(&format!("Rail Center refused the credential ({shown})"))
                && line[0].contains("RAIL_AUTH_MODE and RAIL_AUTH_TOKEN"),
            "{logs}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_dropped_key_is_warned_of_with_its_kid_and_why_whether_or_not_one_is_left() {
    // `y = 2` decodes to no point on the curve.
    let bad =
        json!({"kty": "OKP", "crv": "Ed25519", "kid": "bad", "x": format!("Ag{}", "A".repeat(41))});
    let with_good = {
        let mut set = key_set(&["a"]);
        set["keys"].as_array_mut().unwrap().push(bad.clone());
        set.to_string()
    };
    let alone = json!({ "keys": [bad] }).to_string();

    let kept = log_until(0, move |_| answer(StatusCode::OK, with_good.as_bytes())).await;
    let none = log_until(0, move |_| answer(StatusCode::OK, alone.as_bytes())).await;

    for logs in [&kept, &none] {
        let line = lines_with(logs, "dropping a signing key the fetched set lists");
        assert_eq!(line.len(), 1, "{logs}");
        assert!(
            line[0].contains("WARN")
                && line[0].contains(r#"kid="bad""#)
                && line[0].contains(r#"reason="its x isn't a point on the Ed25519 curve""#),
            "{logs}"
        );
    }
    assert!(
        lines_with(&kept, "holding signing keys")[0].contains(r#"kids="a""#),
        "{kept}"
    );
    assert!(
        lines_with(&none, "fetch failed")[0].contains(r#"reason="the set holds no usable key""#),
        "{none}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_new_key_under_a_kid_held_is_news() {
    // `y = 1`, a point on the curve, so another key than the vectors'.
    let replaced = json!({"keys": [{
        "kty": "OKP", "crv": "Ed25519", "kid": "a", "x": format!("AQ{}", "A".repeat(41)),
    }]})
    .to_string();
    let logs = log_until(300, move |n| match n {
        0 => ok(&["a"]),
        _ => answer(StatusCode::OK, replaced.as_bytes()),
    })
    .await;

    assert_eq!(lines_with(&logs, "holding signing keys").len(), 2, "{logs}");
    assert!(lines_with(&logs, "dropping").is_empty(), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn an_empty_set_is_said_once_at_info_and_never_as_a_failure() {
    let logs = log_until(300, |_| answer(StatusCode::OK, b"{\"keys\": []}")).await;

    let said = lines_with(&logs, "publishes no signing keys");
    assert_eq!(said.len(), 1, "{logs}");
    assert!(said[0].contains("INFO"), "{logs}");
    assert!(!logs.contains("fetch failed"), "{logs}");
}
