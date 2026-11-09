use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use rail_policy::Fallback;
use rail_runtime::{Answer, Call, Gateway, GatewaySettings, UnitSettings};
use serde_json::{Value, json};
use tokio::time::Instant;
use tokio::time::timeout;

use crate::bundles::{self, Script};
use crate::holder::{TestClock, until};
use crate::keys;
use crate::sink::Sink;

// Signed, naming the key `new`, and signed by nobody: only the key it names is read here.
const NAMING_NEW: &str = "eyJhbGciOiAiRWREU0EiLCAia2lkIjogIm5ldyJ9.e30.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
// As `NAMING_NEW`, but `alg: none`.
const NAMING_NEW_UNSIGNABLE: &str = "eyJhbGciOiAibm9uZSIsICJraWQiOiAibmV3In0.e30.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

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
    // Slow, so a wait for a key not held lasts its limit.
    let keys = Script {
        takes: Duration::from_secs(10),
        ..Script::new(clock, |_| keys::ok(&["old"]))
    };
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
    // A bundle is waited for 2 s, or to the deadline if sooner.
    for (method, fallback, deadline, answered, waited) in [
        ("POST", Fallback::Pass, 5, Answer::Forward, 2),
        ("POST", Fallback::Block, 1, Answer::CannotApply, 1),
        // A deadline past any `Instant` reads as a day.
        ("POST", Fallback::Pass, u64::MAX, Answer::Forward, 2),
        ("GET", Fallback::Block, 5, Answer::Forward, 0),
    ] {
        let clock = TestClock::new();
        let settings = GatewaySettings {
            deadline: Duration::from_secs(deadline),
            no_bundle_fallback: fallback,
            ..GatewaySettings::default()
        };
        let never = Script {
            takes: Duration::from_secs(3600),
            ..Script::new(&clock, |_| bundles::failed())
        };
        let started = start(&clock, never, settings);
        until(&clock, 0).await;

        let sent = call(method, None);
        let arrived = Instant::now();
        let (answer, back) = started.gateway.handle(sent.clone()).await;

        assert_eq!(answer, answered, "{method} {fallback:?}");
        assert_eq!(
            arrived.elapsed(),
            Duration::from_secs(waited),
            "{method} {fallback:?}"
        );
        assert_eq!(back.body, sent.body);
    }
}

#[tokio::test(start_paused = true)]
async fn a_key_not_held_is_waited_for_only_where_the_bundle_can_accept_a_signed_ticket() {
    let tenant = json!({"ticket_issuer": "https://rc/tenants/t", "ticket_audience": "t"});
    let issuer_only = json!({"ticket_issuer": "https://rc/tenants/t"});
    let empty = json!({"ticket_issuer": "", "ticket_audience": ""});
    // The key never arrives, so it's waited for 2 s.
    for (with, x_rail, fetches, waited) in [
        (tenant.clone(), Some(NAMING_NEW), 2, 2),
        (json!({}), Some(NAMING_NEW), 1, 0),
        (issuer_only, Some(NAMING_NEW), 1, 0),
        (empty, Some(NAMING_NEW), 1, 0),
        (tenant.clone(), Some(NAMING_NEW_UNSIGNABLE), 1, 0),
        (tenant, None, 1, 0),
    ] {
        let clock = TestClock::new();
        let bundle = denying_all(with.clone());
        let started = start(
            &clock,
            Script::new(&clock, move |_| bundles::ok(&bundle)),
            GatewaySettings::default(),
        );
        // Past the 30 s a refetch for a key not held waits after the last fetch.
        until(&clock, 31).await;

        let arrived = Instant::now();
        started.gateway.handle(call("POST", x_rail)).await;

        assert_eq!(
            started.keys.fetched_at().len(),
            fetches,
            "{with} {x_rail:?}"
        );
        assert_eq!(
            arrived.elapsed(),
            Duration::from_secs(waited),
            "{with} {x_rail:?}"
        );
    }
}

// Over a minute of scans unoptimised, and seconds optimised, unless stopped within one.
const SCANS: u64 = 2_000;
const SCANNED: usize = 16 << 20;

// On real time: a worker holds back paused time, so a deadline can't pass while it runs.
#[tokio::test]
async fn a_call_answered_unjudged_or_dropped_has_its_judgement_stopped() {
    // The gateway's deadline passing, or the host dropping the call at 10 ms.
    for (deadline, dropped) in [
        (Duration::from_millis(10), false),
        (Duration::from_secs(5), true),
    ] {
        let clock = TestClock::new();
        // Each scans all of `x-a` for a `b` it lacks, so a call carrying it is judged slowly.
        let mut policies = Vec::from_iter((1..=SCANS).map(|priority| {
            json!({
                "id": format!("5c8f1e42-0000-4000-9000-{priority:012x}"),
                "name": "S",
                "priority": priority,
                "condition": {"path": "request.headers.x-a", "op": "matches", "value": "*b*"},
                "action": "block",
                "enabled": true,
            })
        }));
        let mut denying = denying_all(json!({}));
        denying["policies"][0]["priority"] = json!(SCANS + 1);
        policies.push(denying["policies"][0].take());
        let bundle = denying_all(json!({"policies": policies}));
        let settings = GatewaySettings {
            deadline,
            max_concurrent_judgements: NonZeroUsize::MIN,
            // Not what a call unjudged with a bundle held gets.
            no_bundle_fallback: Fallback::Block,
            unit: UnitSettings {
                step_limit: u64::MAX,
            },
        };
        let started = start(
            &clock,
            Script::new(&clock, move |_| bundles::ok(&bundle)),
            settings,
        );
        until(&clock, 0).await;
        let huge = Call {
            headers: vec![("x-a".into(), "a".repeat(SCANNED))],
            ..call("POST", None)
        };

        let handled = started.gateway.handle(huge.clone());
        if dropped {
            assert!(timeout(Duration::from_millis(10), handled).await.is_err());
        } else {
            let (answer, back) = handled.await;
            assert_eq!(answer, Answer::Forward);
            assert!(back.headers == huge.headers);
        }

        // Judged once the slot is free, and reported alone.
        let judged =
            async { while started.gateway.handle(call("POST", None)).await.0 != Answer::Deny {} };
        let freed = timeout(Duration::from_secs(5), judged).await;
        assert!(freed.is_ok(), "{deadline:?}");
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert_eq!(started.sink.reports().len(), 1, "{deadline:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_ticket_is_read_at_the_wall_clocks_time() {
    let needs_ticket = json!({"policies": [{
        "id": bundles::ONE,
        "name": "P",
        "priority": 1,
        "condition": {"path": "ticket", "op": "not_existing"},
        "action": "block",
        "enabled": true,
    }]});
    // `{"exp": 1800000000}`, the clock's start, and a second later.
    for (x_rail, answered) in [
        ("eyJleHAiOiAxODAwMDAwMDAwfQ", Answer::Deny),
        ("eyJleHAiOiAxODAwMDAwMDAxfQ", Answer::Forward),
    ] {
        let clock = TestClock::new();
        let bundle = denying_all(needs_ticket.clone());
        let started = start(
            &clock,
            Script::new(&clock, move |_| bundles::ok(&bundle)),
            GatewaySettings::default(),
        );
        until(&clock, 0).await;

        let (answer, _) = started.gateway.handle(call("POST", Some(x_rail))).await;

        assert_eq!(answer, answered, "{x_rail}");
    }
}
