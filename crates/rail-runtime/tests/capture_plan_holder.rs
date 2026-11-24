use std::sync::Arc;
use std::time::Duration;

use rail_runtime::{CapturePlanHolder, Jitter, RailCenterCapturePlans};
use reqwest::StatusCode;
use serde_json::json;
use tokio::runtime::Handle;

use crate::capture_plans::{MINUTE, Script, agent, answer, hash, held, no_plan, ok, plan, start};
use crate::common::{self, Reply, client, response, serve};
use crate::holder::{TestClock, until};
use crate::scripted::{Answering, failed};

// Asked again 5 s after an answer that may pass, the period after any other.
const SOONER: u64 = 5;
const PERIOD: u64 = 60;

fn a() -> String {
    hash('a')
}

fn b() -> String {
    hash('b')
}

// One answer after plan a is applied down each path it can take: the hash held after it, and when
// the next fetch comes.
fn answers() -> Vec<(&'static str, Answering, Option<String>, u64)> {
    let kept = || Some(a());
    vec![
        (
            "another plan",
            || ok(&plan(&b(), json!({}))),
            Some(b()),
            PERIOD,
        ),
        (
            "a plan of another minor",
            || ok(&plan(&b(), json!({"schema_version": "1.3"}))),
            Some(b()),
            PERIOD,
        ),
        // The hash covers every field, so the plan held is unchanged whatever it reads as.
        (
            "the same hash naming another agent",
            || {
                let other = json!({"host_id": "host-7f3c", "sandbox_name": "other"});
                ok(&plan(&a(), json!({"agent": other})))
            },
            kept(),
            PERIOD,
        ),
        ("a 204", no_plan, None, PERIOD),
        (
            "another major",
            || ok(&json!({"schema_version": "2.0", "content_hash": b()})),
            kept(),
            PERIOD,
        ),
        (
            "a plan the proxy refuses",
            || ok(&plan(&b(), json!({"window_seconds": 10}))),
            kept(),
            PERIOD,
        ),
        (
            "another redaction_version",
            || ok(&plan(&b(), json!({"redaction_version": 2}))),
            kept(),
            PERIOD,
        ),
        (
            "a 404",
            || answer(StatusCode::NOT_FOUND, b""),
            kept(),
            PERIOD,
        ),
        (
            "a body that isn't JSON",
            || answer(StatusCode::OK, b"<html>nope</html>"),
            kept(),
            PERIOD,
        ),
        ("a JSON array", || ok(&json!([])), kept(), PERIOD),
        (
            "no content_hash",
            || ok(&plan(&b(), json!({"content_hash": null}))),
            kept(),
            PERIOD,
        ),
        (
            "a content_hash in capitals",
            || ok(&plan(&hash('B'), json!({}))),
            kept(),
            PERIOD,
        ),
        (
            "another agent's plan",
            || {
                let other = json!({"host_id": "host-7f3c", "sandbox_name": "other"});
                ok(&plan(&b(), json!({"agent": other})))
            },
            kept(),
            PERIOD,
        ),
        (
            "a 503",
            || answer(StatusCode::SERVICE_UNAVAILABLE, b""),
            kept(),
            SOONER,
        ),
        ("a deadline", failed, kept(), SOONER),
    ]
}

#[tokio::test(start_paused = true)]
async fn only_a_plan_applied_or_a_204_replaces_the_plan_held() {
    for (label, second, expected, retry) in answers() {
        let clock = TestClock::new();
        let script = Script::new(&clock, move |n| match n {
            0 => ok(&plan(&a(), json!({}))),
            1 => second(),
            _ => no_plan(),
        });

        let holder = start(&script, MINUTE);
        until(&clock, 0).await;
        assert_eq!(held(&holder), Some(a()), "{label}");
        until(&clock, 60).await;
        assert_eq!(held(&holder), expected, "{label}");
        until(&clock, 60 + retry).await;

        assert_eq!(script.fetched_at(), [0, 60, 60 + retry], "{label}");
    }
}

#[tokio::test(start_paused = true)]
async fn with_none_held_only_a_plan_applied_holds_one() {
    for (label, first, expected, _) in answers() {
        let clock = TestClock::new();
        let script = Script::new(&clock, move |_| first());

        let holder = start(&script, MINUTE);
        until(&clock, 0).await;

        let expected = expected.filter(|hash| *hash == b());
        assert_eq!(held(&holder), expected, "{label}");
    }
}

#[tokio::test(start_paused = true)]
async fn the_plan_held_is_replaced_only_when_it_changes() {
    let clock = TestClock::new();
    let answers = vec![
        ok(&plan(&a(), json!({}))),
        // The same hash, a plan refused, a failure, then the same hash again.
        ok(&plan(&a(), json!({}))),
        ok(&plan(&b(), json!({"redaction_version": 2}))),
        answer(StatusCode::SERVICE_UNAVAILABLE, b""),
        ok(&plan(&a(), json!({}))),
        // Another plan; then no plan, twice.
        ok(&plan(&b(), json!({}))),
        no_plan(),
        no_plan(),
    ];
    let script = Script::in_turn(&clock, answers);

    let holder = start(&script, MINUTE);
    let mut snapshots = Vec::new();
    for at in [0, 60, 120, 180, 185, 245, 305, 365] {
        until(&clock, at).await;
        snapshots.push(holder.snapshot());
    }

    let first = snapshots[0].as_ref().unwrap();
    let same =
        |snapshot: &Option<Arc<_>>| snapshot.as_ref().is_some_and(|it| Arc::ptr_eq(first, it));
    assert!(snapshots[1..5].iter().all(same), "the same plan");
    assert!(
        !same(&snapshots[5]) && snapshots[5].is_some(),
        "another plan"
    );
    assert!(snapshots[6].is_none() && snapshots[7].is_none(), "no plan");
}

// Real time: the fetch goes over a socket, which paused time would race.
#[tokio::test]
async fn the_route_the_agent_and_the_credential_travel_with_the_request() {
    let body = plan(&a(), json!({})).to_string();
    let server = serve(move |_| Reply::Raw(response("200 OK", &[], body.as_bytes()))).await;
    let source = RailCenterCapturePlans {
        client: client(&format!("{}/", server.url)),
        agent: agent(),
    };

    let holder = CapturePlanHolder::start(&Handle::current(), source, MINUTE, Jitter::new(|| 1.0));
    let capturing = async {
        while holder.snapshot().is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), capturing)
        .await
        .expect("the plan is held within 10 s");

    let requests = server.requests.lock().unwrap();
    let request = requests[0].to_ascii_lowercase();
    assert!(
        request.starts_with(
            "get /v1/capture-plan?host_id=host-7f3c&sandbox_name=agent-finretail http/1.1\r\n"
        ),
        "{request}"
    );
    assert!(request.contains(&format!(
        "authorization: bearer {}",
        common::TOKEN.to_ascii_lowercase()
    )));
}
