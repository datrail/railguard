use std::sync::Arc;

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use rail_runtime::{Call, Gateway, GatewaySettings, read_path};

use crate::bundles::{self, Script};
use crate::holder::{TestClock, until};
use crate::keys;
use crate::scripted::answer;
use crate::sink::Sink;

const VECTORS: &str = include_str!("../../../vectors/gateway_enforcement_conformance_cases.json");

#[test]
fn the_path_is_the_uri_without_its_query_percent_decoded() {
    let cases = [
        ("/mcp", "/mcp"),
        ("/mcp?session=1", "/mcp"),
        ("/a%20b/%2Fc?x=%20", "/a b//c"),
        // An invalid escape is kept, invalid UTF-8 replaced.
        ("/a%zz", "/a%zz"),
        ("/a%ff", "/a\u{fffd}"),
        ("", "/"),
        ("?only=query", "/"),
    ];
    for (uri, path) in cases {
        assert_eq!(read_path(uri), path, "{uri:?}");
    }
}

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
    // The names cased otherwise, as a host may send them.
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
async fn every_enforcement_case_is_answered_and_reported_as_it_says() {
    let Vectors { cases } = serde_json::from_str(VECTORS).unwrap();
    assert_eq!(cases.len(), 92);
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
