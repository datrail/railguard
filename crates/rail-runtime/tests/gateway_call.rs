use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use rail_policy::Fallback;
use rail_runtime::{Answer, Call, Gateway, GatewaySettings};
use serde_json::{Value, json};

use crate::bundles::{self, Script};
use crate::holder::{TestClock, until};
use crate::keys;
use crate::sink::Sink;

// Signed, naming the key `new`, and signed by nobody: only the key it names is read here.
const NAMING_NEW: &str = "eyJhbGciOiAiRWREU0EiLCAia2lkIjogIm5ldyJ9.e30.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

// At `enforce`, denying every call; `with` replaces or adds root fields.
fn denying_all(with: Value) -> Value {
    let mut bundle = bundles::bundle(
        "v1",
        json!({
            "enforcement": {"mode": "enforce"},
            "binding_fallback": "pass",
            "policies": [{
                "id": bundles::ONE,
                "name": "P",
                "priority": 1,
                "condition": {"path": "request.endpoint_key", "op": "not_existing"},
                "action": "block",
                "enabled": true,
            }],
        }),
    );
    for (name, value) in with.as_object().unwrap() {
        bundle[name] = value.clone();
    }
    bundle
}

fn call(method: &str, x_rail: Option<&str>) -> Call<String> {
    Call {
        path: "/mcp".into(),
        method: method.into(),
        body: r#"{"method": "resources/read"}"#.into(),
        headers: x_rail
            .map(|ticket| ("x-rail".into(), ticket.into()))
            .into_iter()
            .collect(),
    }
}

// A gateway on `bundles`, its reports, and the key source it fetches from.
struct Started {
    gateway: Gateway,
    sink: Sink,
    keys: Script,
}

fn start(clock: &TestClock, bundles: Script, settings: GatewaySettings) -> Started {
    let keys = Script::new(clock, |_| keys::ok(&["old"]));
    let sink = Sink::accepting();
    let gateway = Gateway::new(
        Arc::new(bundles::start(&bundles, bundles::MINUTE)),
        Arc::new(keys::start(clock, &keys)),
        Arc::new(sink.start()),
        clock.wall(),
        settings,
    );
    Started {
        gateway,
        sink,
        keys,
    }
}

#[tokio::test(start_paused = true)]
async fn with_no_bundle_a_post_takes_the_setting_and_any_other_call_is_forwarded() {
    for (method, fallback, answered) in [
        ("POST", Fallback::Pass, Answer::Forward),
        ("POST", Fallback::Block, Answer::CannotApply),
        ("GET", Fallback::Block, Answer::Forward),
    ] {
        let clock = TestClock::new();
        let settings = GatewaySettings {
            no_bundle_fallback: fallback,
            ..GatewaySettings::default()
        };
        let started = start(&clock, Script::new(&clock, |_| bundles::failed()), settings);
        until(&clock, 0).await;

        let sent = call(method, None);
        let (answer, back) = started.gateway.handle(sent.clone()).await;

        assert_eq!(answer, answered, "{method} {fallback:?}");
        assert_eq!(back.body, sent.body);
    }
}

#[tokio::test(start_paused = true)]
async fn a_key_not_held_is_waited_for_only_where_the_bundle_can_accept_a_signed_ticket() {
    let tenant = json!({"ticket_issuer": "https://rc/tenants/t", "ticket_audience": "t"});
    for (with, fetches) in [(tenant, 2), (json!({}), 1)] {
        let clock = TestClock::new();
        let bundle = denying_all(with.clone());
        let started = start(
            &clock,
            Script::new(&clock, move |_| bundles::ok(&bundle)),
            GatewaySettings::default(),
        );
        // Past the 30 s a refetch for a key not held waits after the last fetch.
        until(&clock, 31).await;

        started.gateway.handle(call("POST", Some(NAMING_NEW))).await;

        assert_eq!(started.keys.fetched_at().len(), fetches, "{with}");
    }
}

// On real time: a worker holds back paused time, so a deadline can't pass while it runs.
#[tokio::test]
async fn past_its_deadline_a_call_is_answered_unjudged_its_judgement_stopped() {
    let clock = TestClock::new();
    let bundle = denying_all(json!({}));
    let settings = GatewaySettings {
        deadline: Duration::from_millis(10),
        places: NonZeroUsize::MIN,
        ..GatewaySettings::default()
    };
    let started = start(
        &clock,
        Script::new(&clock, move |_| bundles::ok(&bundle)),
        settings,
    );
    until(&clock, 0).await;
    // Read in about 40 ms optimised, and far longer unoptimised.
    let huge = Call {
        body: format!(
            r#"{{"method": "resources/read", "q": "{}"}}"#,
            "a".repeat(64 << 20)
        ),
        ..call("POST", None)
    };

    let (answer, back) = started.gateway.handle(huge.clone()).await;
    assert_eq!(answer, Answer::Forward);
    assert!(back.body == huge.body);

    // Unjudged while the stopped judgement holds the one place, then judged.
    let mut answer = Answer::Forward;
    for _ in 0..200 {
        answer = started.gateway.handle(call("POST", None)).await.0;
        if answer == Answer::Deny {
            break;
        }
    }
    assert_eq!(answer, Answer::Deny);
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(started.sink.reports().len(), 1);
}
