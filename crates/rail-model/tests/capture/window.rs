use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_model::body::{BodyRead, Unread};
use rail_model::capture::{
    BodyUnreadReason, Document, ObservedCall, Window, compile_plan, parse_plan,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::patch;
use crate::keep_body;

const VECTORS: &str = include_str!("../../../../vectors/capture_window_conformance_cases.json");

#[derive(Deserialize)]
struct Vectors {
    plan: Value,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    patch: Vec<Value>,
    start: u64,
    end: u64,
    calls: Vec<Received>,
    #[serde(default)]
    calls_dropped: u64,
    sent: Sent,
    expect: Value,
}

#[derive(Deserialize)]
struct Received {
    at: u64,
    call: Call,
}

#[derive(Deserialize)]
struct Call {
    authority: String,
    method: String,
    path: String,
    body: Value,
}

#[derive(Deserialize)]
struct Sent {
    document_id: Uuid,
    generated_at: u64,
    windows_dropped: u64,
}

fn time(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

// `<N bytes>` stands for N bytes of `x`, as the capture design writes a body; `bytes` pads the
// body with spaces, which JSON allows after its value.
fn expand(text: &str, bytes: Option<u64>) -> String {
    let mut expanded = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        let (before, marker) = rest.split_at(at);
        expanded.push_str(before);
        let end = marker.find(" bytes>").unwrap();
        expanded.push_str(&"x".repeat(marker[1..end].parse().unwrap()));
        rest = &marker[end + " bytes>".len()..];
    }
    expanded.push_str(rest);
    let padding = bytes.map_or(0, |bytes| bytes as usize - expanded.len());
    expanded + &" ".repeat(padding)
}

fn observe(call: &Call) -> ObservedCall {
    let (body, body_bytes) = match (&call.body["text"], &call.body["unread"]) {
        (Value::String(text), _) => {
            let text = expand(text, call.body["bytes"].as_u64());
            (keep_body(text.as_bytes()), text.len() as u64)
        }
        (_, Value::String(why)) => {
            let why = match why.as_str() {
                "not_sent" => Unread::NotSent,
                other => panic!("{other}"),
            };
            (BodyRead::Unread(why), 0)
        }
        _ => panic!("{}", call.body),
    };
    ObservedCall {
        authority: call.authority.clone().into_bytes(),
        method: call.method.clone().into_bytes(),
        path: call.path.clone().into_bytes(),
        body,
        body_bytes,
    }
}

// What the sender gives a document.
fn close(window: Window, end: u64, sent: &Sent) -> Document {
    let generated_at = time(sent.generated_at);
    let closed = window.close(time(end));
    closed.into_document(sent.document_id, generated_at, sent.windows_dropped)
}

#[test]
fn test_vectors() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());
    let mut failures = Vec::new();
    for case in &vectors.cases {
        let mut plan = vectors.plan.clone();
        case.patch
            .iter()
            .for_each(|operation| patch(&mut plan, operation));
        let compiled = Arc::new(compile_plan(parse_plan(&plan.to_string()).unwrap()));
        let mut window = Window::open(Arc::clone(&compiled), time(case.start));
        window.count_dropped(case.calls_dropped);
        for received in &case.calls {
            window.record(&observe(&received.call), time(received.at));
        }
        let document = close(window, case.end, &case.sent);
        let written = serde_json::to_value(&document).unwrap();
        if written != case.expect {
            failures.push(format!("{}: {written}", case.name));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn test_tells_why_a_body_went_unread() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    let compiled = Arc::new(compile_plan(parse_plan(&vectors.plan.to_string()).unwrap()));
    let mut window = Window::open(Arc::clone(&compiled), time(0));
    let call = |body: BodyRead| ObservedCall {
        authority: b"api.github.com".to_vec(),
        method: b"POST".to_vec(),
        path: b"/mcp".to_vec(),
        body,
        body_bytes: 0,
    };
    let ambiguous = br#"{"method": "tools/call", "params": {"name": "a", "Name": "b"}}"#;
    let cases = [
        (
            BodyRead::Unread(Unread::PastCap),
            Some(BodyUnreadReason::PastCap),
        ),
        (keep_body(b"[]"), Some(BodyUnreadReason::Batch)),
        (keep_body(b"{"), Some(BodyUnreadReason::NotJson)),
        (keep_body(ambiguous), Some(BodyUnreadReason::Ambiguous)),
        (keep_body(b"{}"), None),
    ];
    for (body, reason) in cases {
        assert_eq!(window.record(&call(body), time(1)), reason);
    }
}
