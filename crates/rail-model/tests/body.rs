mod mcp;
mod tree;

use rail_model::body::mcp::McpReading;
use rail_model::body::read_endpoint;
use rail_model::body::{BodyBuffer, ByteBudget, RequestBody, Unkept, Unparsed};
use rail_model::endpoint::{Endpoint, Resolution};
use serde_json::{Value, json};

const MOUNT: &str = "/mcp";

fn read(body: &Value) -> Resolution {
    read_endpoint(MOUNT, body.to_string().as_bytes())
}

fn build_call(method: &str, tool_name: Option<&str>) -> Value {
    match tool_name {
        Some(name) => {
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": {"name": name}})
        }
        None => json!({"jsonrpc": "2.0", "id": 1, "method": method}),
    }
}

fn get_resolved(text: &str) -> Resolution {
    Resolution::Judged(Endpoint::Resolved(text.parse().unwrap()))
}

const UNRECOGNISED: Resolution = Resolution::Judged(Endpoint::Unrecognised);
const KEYLESS: Resolution = Resolution::Judged(Endpoint::Keyless);

// A tool name is text, made safe where it's logged.
#[test]
fn test_uses_every_part_of_the_key_verbatim() {
    for tool_name in [
        "Track_Package",
        "orders.v2",
        "track\npackage",
        "track\u{1b}package",
        "track\u{202e}package",
        "track\u{2028}package",
    ] {
        assert_eq!(
            read(&build_call("tools/call", Some(tool_name))),
            get_resolved(&format!("/mcp#tools/call#{tool_name}")),
            "{tool_name:?}"
        );
    }
}

#[test]
fn test_forwards_a_session_or_discovery_message_unjudged() {
    let methods = [
        "initialize",
        "ping",
        "notifications/initialized",
        "notifications/cancelled",
        "tools/list",
        "prompts/list",
        "resources/list",
        "resources/templates/list",
    ];
    for method in methods {
        assert_eq!(
            read(&build_call(method, None)),
            Resolution::Discovery,
            "{method}"
        );
    }
}

#[test]
fn test_reads_every_other_method_as_keyless() {
    for method in [
        "resources/read",
        "prompts/get",
        "completion/complete",
        "notificationsx",
    ] {
        assert_eq!(
            read(&build_call(method, Some("track_package"))),
            KEYLESS,
            "{method}"
        );
    }
}

#[test]
fn test_reads_a_missing_method_as_unrecognised() {
    assert_eq!(read(&json!({"params": {"name": "a"}})), UNRECOGNISED);
    assert_eq!(read(&build_call("", None)), UNRECOGNISED);
}

#[test]
fn test_reads_a_tools_call_naming_no_tool_as_unrecognised() {
    for tool_name in [None, Some("")] {
        assert_eq!(
            read(&build_call("tools/call", tool_name)),
            UNRECOGNISED,
            "{tool_name:?}"
        );
    }
}

#[test]
fn test_reads_a_key_past_rail_centers_cap_in_characters_as_unrecognised() {
    for c in ["t", "é"] {
        let fits = c.repeat(255 - "/mcp#tools/call#".len());
        assert_eq!(
            read(&build_call("tools/call", Some(&fits))),
            get_resolved(&format!("/mcp#tools/call#{fits}")),
            "{c}"
        );
        assert_eq!(
            read(&build_call("tools/call", Some(&format!("{fits}{c}")))),
            UNRECOGNISED,
            "{c}"
        );
    }
}

#[test]
fn test_reads_a_key_on_an_empty_path_as_unrecognised() {
    let body = build_call("tools/call", Some("track_package")).to_string();
    assert_eq!(read_endpoint("", body.as_bytes()), UNRECOGNISED);
}

#[test]
fn test_reads_what_the_body_names_as_json_readers_would() {
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
    let resolved = get_resolved("/mcp#tools/call#a");
    for (body, named) in [
        (deep(256).into_bytes(), &resolved),
        (deep(257).into_bytes(), &UNRECOGNISED),
        (quoted.into_bytes(), &resolved),
        (
            r#"{"method": 1e400, "method": "tools/call", "params": {"name": "c"}, "params": {"name": 1e400, "name": "a"}}"#
                .into(),
            &resolved,
        ),
        (r#"{"method": "tools/call", "params": "a\/b"}"#.into(), &UNRECOGNISED),
        // Keys written with escapes, read as what they spell.
        (br#"{"\u006dethod": "tools/call", "params": {"\u006eame": "a"}}"#.to_vec(), &resolved),
        // Keys written with escapes, read as what they spell.
        (br#"{"method": "tools/call", "params": {"name": "a"}}"#.to_vec(), &resolved),
        // Keys a string can't hold, read past.
        (r#"{"\ud800": 0, "method": "tools/call", "params": {"\udfff": 1, "name": "a"}}"#.into(), &resolved),
        (r#"{"method": "tools/call", "params": {"name": "a"}} {}"#.into(), &UNRECOGNISED),
        (
            "\u{feff}{\"method\": \"tools/call\", \"params\": {\"name\": \"a\"}}".into(),
            &resolved,
        ),
        (r#"{"method": "tools/call", "params": {"name": 5}}"#.into(), &UNRECOGNISED),
        (r#"[{"method": "tools/call", "params": {"name": "a"}}]"#.into(), &UNRECOGNISED),
        // Not UTF-8.
        (b"{\"method\": \"tools/call\", \"params\": {\"name\": \"a\xff\"}}".into(), &UNRECOGNISED),
        // A method naming no tool keeps its key-less reading whatever `params` holds.
        (r#"{"method": "resources/read", "params": [1]}"#.into(), &KEYLESS),
        (r#"{"method": "resources/read", "params": 1e400}"#.into(), &KEYLESS),
    ] {
        assert_eq!(
            &read_endpoint(MOUNT, &body),
            named,
            "{:.60}",
            String::from_utf8_lossy(&body)
        );
    }
}

// A body kept whole, with no cap or budget to meet.
fn keep(body: &[u8]) -> RequestBody {
    let mut buffer = BodyBuffer::new(usize::MAX, &ByteBudget::new(usize::MAX));
    buffer.append(body);
    buffer.finish().unwrap()
}

#[test]
fn test_keeps_a_body_up_to_its_cap_counting_every_byte() {
    let budget = ByteBudget::new(usize::MAX);
    let mut whole = BodyBuffer::new(5, &budget);
    whole.append(b"abc");
    whole.append(b"de");
    assert_eq!(whole.received_bytes(), 5);
    assert_eq!(whole.finish().unwrap().as_bytes(), b"abcde");

    let mut past = BodyBuffer::new(5, &budget);
    for chunk in [&b"abc"[..], b"def", b"g"] {
        past.append(chunk);
    }
    assert_eq!(past.received_bytes(), 7);
    assert_eq!(past.finish().unwrap_err(), Unkept::PastCap);
}

#[test]
fn test_shares_one_budget_between_bodies_until_each_is_dropped() {
    let budget = ByteBudget::new(10);
    let kept = |bytes: usize| {
        let mut buffer = BodyBuffer::new(usize::MAX, &budget);
        buffer.append(&vec![b'0'; bytes]);
        buffer.finish()
    };
    let first = kept(8).unwrap();
    assert_eq!(kept(4).unwrap_err(), Unkept::PastBudget);
    drop(first);
    assert!(kept(4).is_ok());

    // A body refused gives its bytes back at once, not when it's dropped.
    let mut refused = BodyBuffer::new(usize::MAX, &budget);
    refused.append(&[b'0'; 8]);
    refused.append(&[b'0'; 8]);
    assert!(kept(10).is_ok());
    assert_eq!(refused.finish().unwrap_err(), Unkept::PastBudget);
}

#[test]
fn test_charges_a_bodys_tree_to_the_budget_too() {
    let body = b"[0, 0, 0]";
    let kept = |budget: &std::sync::Arc<ByteBudget>, bytes: &[u8]| {
        let mut buffer = BodyBuffer::new(usize::MAX, budget);
        buffer.append(bytes);
        buffer.finish()
    };
    let tight = ByteBudget::new(body.len());
    let unparsed = kept(&tight, body).unwrap();
    assert_eq!(unparsed.root().err(), Some(Unparsed::PastBudget));
    assert_eq!(unparsed.mcp(), &McpReading::Unreadable);

    let roomy = ByteBudget::new(1 << 20);
    let parsed = kept(&roomy, body).unwrap();
    assert!(parsed.root().is_ok());
    let left = (1 << 20) - body.len();
    assert_eq!(
        kept(&roomy, &vec![b'0'; left]).unwrap_err(),
        Unkept::PastBudget
    );
    drop(parsed);
    assert!(kept(&roomy, &vec![b'0'; left]).is_ok());
}
