use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_model::agent::AgentId;
use rail_model::body::{BodyRead, Unread};
use rail_model::capture::{
    BodyUnreadReason, Document, NotCapturing, ObservedCall, PlanId, Window, compile_plan,
    parse_plan,
};
use serde::Deserialize;
use serde_json::{Value, json};
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

// A document's fields are written in the field reference's order, a record's keys and measures in
// the plan's, which a reader may rely on and a comparison of values can't see.
#[test]
fn test_writes_the_fields_in_their_order() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    let case = &vectors.cases[0];
    let compiled = Arc::new(compile_plan(parse_plan(&vectors.plan.to_string()).unwrap()));
    let mut window = Window::open(Arc::clone(&compiled), time(case.start));
    for received in &case.calls {
        window.record(&observe(&received.call), time(received.at));
    }
    let document = close(window, case.end, &case.sent);
    let written = serde_json::to_string(&document).unwrap();
    let in_order = |fields: &[&str], from: &str| {
        let from = written.find(from).unwrap();
        let at: Vec<usize> = fields
            .iter()
            .map(|field| from + written[from..].find(&format!("\"{field}\"")).unwrap())
            .collect();
        assert!(at.is_sorted(), "{fields:?} in {written}");
    };
    let document = [
        "schema_version",
        "generated_at",
        "document_id",
        "agent",
        "status",
        "plan_version",
        "plan_hash",
        "redaction_version",
        "window",
        "truncated",
        "hosts",
        "records",
        "sequence",
    ];
    in_order(&document, "{");
    let truncated = [
        "calls_coarsened",
        "calls_dropped",
        "entries_skipped",
        "body_unread",
        "windows_dropped",
        "path_unresolved",
        "values_replaced",
        "values_ambiguous",
        "steps_dropped",
    ];
    in_order(&truncated, "\"truncated\"");
    let record = [
        "record_id",
        "entry_id",
        "context_keys",
        "field_types",
        "measures",
    ];
    in_order(&record, "\"records\"");
    let catch_all = "\"entry_id\":\"catch_all\"";
    in_order(&["host", "path_prefix", "method", "operation"], catch_all);
    in_order(&["call_count", "error_count", "request_bytes"], catch_all);
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

#[test]
fn test_writes_a_window_that_captured_nothing() {
    let agent = AgentId {
        host_id: "host-7f3c".into(),
        sandbox_name: "agent-finretail".into(),
    };
    // A time is written to the nanosecond the clock gives.
    let generated_at = time(900) + Duration::from_millis(999);
    let plan = |plan_version| PlanId {
        plan_hash: "sha256:ab".into(),
        plan_version,
    };
    let write = |why| {
        let start = time(0) + Duration::from_millis(500);
        let document = Document::not_capturing(
            why,
            agent.clone(),
            start,
            time(900),
            Uuid::nil(),
            generated_at,
            1,
        );
        serde_json::to_value(document).unwrap()
    };
    let truncated = json!({"calls_coarsened": 0, "calls_dropped": 0, "entries_skipped": 0,
        "body_unread": 0, "windows_dropped": 1, "path_unresolved": 0, "values_replaced": 0,
        "values_ambiguous": 0});
    let no_plan = json!({"schema_version": "1.0", "generated_at": "1970-01-01T00:15:00.999Z",
        "document_id": "00000000-0000-0000-0000-000000000000",
        "agent": {"host_id": "host-7f3c", "sandbox_name": "agent-finretail"},
        "status": "no_plan", "redaction_version": 1,
        "window": {"start": "1970-01-01T00:00:00.5Z", "end": "1970-01-01T00:15:00Z"},
        "truncated": truncated, "hosts": [], "records": []});
    assert_eq!(write(NotCapturing::NoPlan), no_plan);
    // A plan held is named, its version where the peek read one.
    let named = |document: Value| {
        let field = |name: &str| document.get(name).cloned();
        (field("status"), field("plan_hash"), field("plan_version"))
    };
    let hash = Some(json!("sha256:ab"));
    for (why, status, version) in [
        (
            NotCapturing::PlanUnknown(plan(Some(4))),
            "plan_unknown",
            Some(json!(4)),
        ),
        (NotCapturing::PlanInvalid(plan(None)), "plan_invalid", None),
        (
            NotCapturing::RedactionMismatch(plan(Some(4))),
            "redaction_mismatch",
            Some(json!(4)),
        ),
    ] {
        assert_eq!(
            named(write(why)),
            (Some(json!(status)), hash.clone(), version)
        );
    }
}

// A wall clock stepped back writes no step before the one received earlier, nor a window ending
// before its last call.
#[test]
fn test_keeps_times_in_order_when_the_clock_steps_back() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    let compiled = Arc::new(compile_plan(parse_plan(&vectors.plan.to_string()).unwrap()));
    let mut window = Window::open(Arc::clone(&compiled), time(100));
    let call = || ObservedCall {
        authority: b"api.github.com".to_vec(),
        method: b"GET".to_vec(),
        path: b"/".to_vec(),
        body: keep_body(b""),
        body_bytes: 0,
    };
    for at in [50, 120, 110] {
        window.record(&call(), time(at));
    }
    let closed = window.close(time(20));
    let document = closed.into_document(Uuid::nil(), time(130), 0);
    let document = serde_json::to_value(document).unwrap();
    let at: Vec<&Value> = document["sequence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| &step["at"])
        .collect();
    let written = |seconds| {
        json!(format!(
            "1970-01-01T00:0{}:{:02}Z",
            seconds / 60,
            seconds % 60
        ))
    };
    assert_eq!(at, [&written(100), &written(120), &written(120)]);
    assert_eq!(document["window"]["end"], written(120));
}
