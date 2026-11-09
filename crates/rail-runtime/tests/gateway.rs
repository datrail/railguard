use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use rail_runtime::{Call, UnitSettings, judge_call};

use crate::bundles::{self, Script};
use crate::holder::{TestClock, until};
use crate::keys;
use crate::scripted::answer;
use crate::sink::Sink;

const VECTORS: &str = include_str!("../../../vectors/gateway_enforcement_conformance_cases.json");

// Between every ticket's `nbf` and its `exp`.
const NOW_EPOCH_SECONDS: i64 = 1_800_000_000;

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    bundle: Option<Value>,
    key_set: Option<Value>,
    request: Request,
    expect: Expect,
}

#[derive(Deserialize)]
struct Request {
    method: String,
    path: String,
    body: String,
    x_rail: Vec<String>,
    x_rail_status: Vec<String>,
    headers: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct Expect {
    status: Option<u16>,
    refusal_body: Option<String>,
    report: Option<Value>,
}

impl Request {
    // As the Python servicer's table test sends it: `x-rail`, then `x-rail-status`, then the rest.
    fn into_call(self) -> Call {
        let headers = (self
            .x_rail
            .into_iter()
            .map(|value| ("x-rail".to_owned(), value)))
        .chain((self.x_rail_status.into_iter()).map(|value| ("x-rail-status".to_owned(), value)))
        .chain(self.headers)
        .collect();
        Call {
            path: self.path,
            method: self.method,
            body: self.body,
            headers,
        }
    }
}

#[tokio::test(start_paused = true)]
async fn every_case_of_the_python_gateway_answers_and_reports_as_it_does() {
    let Vectors { cases } = serde_json::from_str(VECTORS).unwrap();
    let mut differing = Vec::new();
    for case in cases {
        // The host forwards both before a call reaches the role.
        let (Some(bundle), "POST") = (&case.bundle, case.request.method.as_str()) else {
            assert!(case.expect.status.is_none(), "{}", case.name);
            continue;
        };
        let clock = TestClock::new();
        let body = bundle.to_string();
        let script = Script::new(&clock, move |_| answer(StatusCode::OK, body.as_bytes()));
        let bundles = bundles::start(&script, bundles::MINUTE);
        let key_set = case.key_set.as_ref().map(|key_set| {
            let body = key_set.to_string();
            let script = Script::new(&clock, move |_| answer(StatusCode::OK, body.as_bytes()));
            keys::start(&clock, &script)
        });
        until(&clock, 0).await;
        let Some(held) = bundles.snapshot() else {
            differing.push(format!("{}: the bundle isn't held", case.name));
            continue;
        };
        let sink = Sink::accepting();
        let reporter = sink.start();

        let answer = judge_call(
            &case.request.into_call(),
            &held,
            key_set.and_then(|holder| holder.keys()).as_deref(),
            NOW_EPOCH_SECONDS,
            &UnitSettings::default(),
            &|denial| reporter.report(denial),
        );

        let refusal = answer
            .refusal()
            .map(|(status, body)| (status, body.to_owned()));
        let expected = case.expect.status.zip(case.expect.refusal_body);
        if refusal != expected {
            differing.push(format!("{}: {answer:?}", case.name));
        }
        // Long enough for a report to reach the sink, or one not expected to show.
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        let mut reports = sink.reports();
        for report in &mut reports {
            report.as_object_mut().unwrap().remove("denied_at");
        }
        if reports != Vec::from_iter(case.expect.report.clone()) {
            differing.push(format!("{}: reported {reports:?}", case.name));
        }
    }
    assert!(differing.is_empty(), "{}", differing.join("\n"));
}

#[tokio::test(start_paused = true)]
async fn a_body_names_what_the_python_gateway_reads_it_to_name() {
    let deep = |depth: usize| {
        let inner = "[".repeat(depth - 3) + &"]".repeat(depth - 3);
        format!(
            r#"{{"method": "tools/call", "params": {{"name": "a", "arguments": {{"q": {inner}}}}}}}"#
        )
    };
    let quoted = format!(
        r#"{{"method": "tools/call", "params": {{"name": "a", "q": "{}"}}}}"#,
        "[".repeat(300)
    );
    let resolved = Some("/mcp#tools/call#a");
    for (body, named) in [
        (deep(256), resolved),
        (deep(257), None),
        (quoted, resolved),
        (
            r#"{"method": "x", "method": "tools/call", "params": {"name": "b", "name": "a"}}"#
                .into(),
            resolved,
        ),
        (
            r#"{"method": "tools\/call", "params": {"name": "a"}}"#.into(),
            resolved,
        ),
        (
            "\u{feff}{\"method\": \"tools/call\", \"params\": {\"name\": \"a\"}}".into(),
            resolved,
        ),
        (
            r#"{"method": "tools/call", "params": {"name": 5}}"#.into(),
            None,
        ),
        (
            r#"[{"method": "tools/call", "params": {"name": "a"}}]"#.into(),
            None,
        ),
    ] {
        let reported = report_for(&body).await;
        assert_eq!(reported["endpoint_key"].as_str(), named, "{body:.60}");
    }
    // A method naming no tool keeps its key-less reading whatever `params` holds.
    let reported = report_for(r#"{"method": "resources/read", "params": [1]}"#).await;
    assert_eq!(reported["metadata"]["endpoint_resolution"], "keyless");
}

// The report of `body` sent to a bundle denying every call.
async fn report_for(body: &str) -> Value {
    let clock = TestClock::new();
    let denies_all = bundles::bundle(
        "v1",
        serde_json::json!({
            "enforcement": {"mode": "enforce"},
            "binding_fallback": "pass",
            "policies": [{
                "id": bundles::ONE,
                "name": "P",
                "priority": 1,
                "condition": {"path": "ticket", "op": "not_existing"},
                "action": "block",
                "enabled": true,
            }],
        }),
    );
    let bundles = bundles::start(
        &Script::new(&clock, move |_| bundles::ok(&denies_all)),
        bundles::MINUTE,
    );
    until(&clock, 0).await;
    let sink = Sink::accepting();
    let reporter = sink.start();
    let call = Call {
        path: "/mcp".into(),
        method: "POST".into(),
        body: body.into(),
        headers: Vec::new(),
    };
    judge_call(
        &call,
        &bundles.snapshot().unwrap(),
        None,
        NOW_EPOCH_SECONDS,
        &UnitSettings::default(),
        &|denial| reporter.report(denial),
    );
    sink.until_sent(1).await;
    sink.reports().remove(0)
}
