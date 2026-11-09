use std::sync::Arc;

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use rail_runtime::{Call, Gateway, GatewaySettings};

use crate::bundles::{self, Script};
use crate::holder::{TestClock, until};
use crate::keys;
use crate::scripted::answer;
use crate::sink::Sink;

const VECTORS: &str = include_str!("../../../vectors/gateway_enforcement_conformance_cases.json");

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
    // In the Python servicer's table test's order, the names cased otherwise, as a host may send them.
    fn into_call(self) -> Call<String> {
        let headers = (self
            .x_rail
            .into_iter()
            .map(|value| ("X-Rail".to_owned(), value)))
        .chain((self.x_rail_status.into_iter()).map(|value| ("X-Rail-Status".to_owned(), value)))
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
    assert_eq!(cases.len(), 88);
    let mut differing = Vec::new();
    for case in cases {
        let clock = TestClock::new();
        let sink = Sink::accepting();
        let gateway = start(&clock, case.bundle.as_ref(), case.key_set.as_ref(), &sink);
        until(&clock, 0).await;

        let (answer, _) = gateway.handle(case.request.into_call()).await;

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

// A gateway holding what Rail Center hands out, none where it hands out nothing.
fn start(
    clock: &TestClock,
    bundle: Option<&Value>,
    key_set: Option<&Value>,
    sink: &Sink,
) -> Gateway {
    let answering = |held: Option<&Value>| {
        let body = held.map(Value::to_string);
        Script::new(clock, move |_| match &body {
            Some(body) => answer(StatusCode::OK, body.as_bytes()),
            None => bundles::failed(),
        })
    };
    Gateway::new(
        Arc::new(bundles::start(&answering(bundle), bundles::MINUTE)),
        Arc::new(keys::start(clock, &answering(key_set))),
        Arc::new(sink.start()),
        clock.wall(),
        GatewaySettings::default(),
    )
}

#[tokio::test(start_paused = true)]
async fn a_body_names_what_the_python_gateway_reads_it_to_name() {
    let deep = |depth: usize| {
        let inner = "[".repeat(depth - 3) + &"]".repeat(depth - 3);
        format!(
            r#"{{"method": "tools\/call", "params": {{"name": "a", "arguments": {{"q": {inner}, "r": {inner}}}}}}}"#
        )
    };
    let quoted = format!(
        r#"{{"method": "tools/call", "params": {{"name": "a", "q": "\"{}"}}}}"#,
        "[".repeat(300)
    );
    let resolved = Some("/mcp#tools/call#a");
    for (body, named) in [
        (deep(256), resolved),
        (deep(257), None),
        (quoted, resolved),
        (
            r#"{"method": 1e400, "method": "tools/call", "params": {"name": "c"}, "params": {"name": 1e400, "name": "a"}}"#
                .into(),
            resolved,
        ),
        (r#"{"method": "tools/call", "params": "a\/b"}"#.into(), None),
        (r#"{"method": "tools/call", "params": {"name": "a"}} {}"#.into(), None),
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
        let reported = report_for(body.as_bytes()).await;
        assert_eq!(reported["endpoint_key"].as_str(), named, "{body:.60}");
    }
    // Not UTF-8: Python's `json.loads` can't decode it either.
    let reported =
        report_for(b"{\"method\": \"tools/call\", \"params\": {\"name\": \"a\xff\"}}").await;
    assert_eq!(reported["metadata"]["endpoint_resolution"], "unrecognised");
    // A method naming no tool keeps its key-less reading whatever `params` holds.
    for params in ["[1]", "1e400"] {
        let reported = report_for(format!(
            r#"{{"method": "resources/read", "params": {params}}}"#
        ))
        .await;
        assert_eq!(
            reported["metadata"]["endpoint_resolution"], "keyless",
            "{params}"
        );
    }
}

// The report of `body` sent to a gateway denying every call.
async fn report_for(body: impl AsRef<[u8]>) -> Value {
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
    let sink = Sink::accepting();
    let gateway = start(&clock, Some(&denies_all), None, &sink);
    until(&clock, 0).await;
    let call = Call {
        path: "/mcp".into(),
        method: "POST".into(),
        body: body.as_ref().to_vec(),
        headers: Vec::new(),
    };
    gateway.handle(call).await;
    sink.until_sent(1).await;
    sink.reports().remove(0)
}
