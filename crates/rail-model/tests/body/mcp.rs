use rail_model::body::mcp::{McpReading, read_endpoint};
use rail_model::endpoint::{Endpoint, Resolution};
use serde_json::{Value, json};

use super::keep;

fn build_reading(method: Option<&str>, tool: Option<&str>) -> McpReading {
    McpReading::Read {
        method: method.map(str::to_string),
        tool: tool.map(str::to_string),
    }
}

#[test]
fn test_reads_the_method_and_tool_a_body_names() {
    for (body, reading) in [
        (
            r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "a"}}"#,
            build_reading(Some("tools/call"), Some("a")),
        ),
        // A tool only on a `tools/call`.
        (
            r#"{"method": "resources/read", "params": {"name": "a"}}"#,
            build_reading(Some("resources/read"), None),
        ),
        (
            r#"{"method": 1, "params": {"name": "a"}}"#,
            build_reading(None, None),
        ),
        (
            r#"{"method": "tools/call", "params": {"name": 1}}"#,
            build_reading(Some("tools/call"), None),
        ),
        (
            r#"{"method": "tools/call", "params": ["a"]}"#,
            build_reading(Some("tools/call"), None),
        ),
        // An empty body (an event stream's `GET`), or JSON that isn't JSON-RPC, names nothing.
        ("", build_reading(None, None)),
        (r#"{"jsonrpc": "2.0"}"#, build_reading(None, None)),
        ("5", build_reading(None, None)),
        // What these name can't be told.
        (
            r#"[{"method": "tools/call", "params": {"name": "a"}}]"#,
            McpReading::Unreadable,
        ),
        ("not json", McpReading::Unreadable),
        (" ", McpReading::Unreadable),
        // Repeated, letter case ignored: readers differ on which they keep.
        (
            r#"{"Method": "ping", "method": "tools/call", "params": {"name": "a"}}"#,
            McpReading::Unreadable,
        ),
        (
            r#"{"method": "tools/call", "params": {"name": "c"}, "params": {"name": "a"}}"#,
            McpReading::Unreadable,
        ),
        (
            r#"{"method": "tools/call", "params": {"name": "c", "NAME": "a"}}"#,
            McpReading::Unreadable,
        ),
        (
            r#"{"method": "resources/read", "params": {"arguments": {}, "Arguments": {}}}"#,
            McpReading::Unreadable,
        ),
        // Only in another case: not the key read.
        (
            r#"{"Method": "tools/call", "params": {"name": "a"}}"#,
            build_reading(None, None),
        ),
    ] {
        assert_eq!(McpReading::read(body.as_bytes()), reading, "{body}");
        assert_eq!(keep(body.as_bytes()).mcp(), &reading, "{body}");
    }
}

const MOUNT: &str = "/mcp";

fn read(body: &Value) -> Resolution {
    read_endpoint(MOUNT, &McpReading::read(body.to_string().as_bytes()))
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
    assert_eq!(
        read_endpoint("", &McpReading::read(body.as_bytes())),
        UNRECOGNISED
    );
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
            r#"{"method": "tools/call", "params": "a\/b"}"#.into(),
            &UNRECOGNISED,
        ),
        // Keys written with escapes, read as what they spell.
        (
            br#"{"\u006dethod": "tools/call", "params": {"\u006eame": "a"}}"#.to_vec(),
            &resolved,
        ),
        // Keys a string can't hold, read past.
        (
            r#"{"\ud800": 0, "method": "tools/call", "params": {"\udfff": 1, "name": "a"}}"#.into(),
            &resolved,
        ),
        (
            r#"{"method": "tools/call", "params": {"name": "a"}} {}"#.into(),
            &UNRECOGNISED,
        ),
        (
            "\u{feff}{\"method\": \"tools/call\", \"params\": {\"name\": \"a\"}}".into(),
            &resolved,
        ),
        (
            r#"{"method": "tools/call", "params": {"name": 5}}"#.into(),
            &UNRECOGNISED,
        ),
        (
            r#"[{"method": "tools/call", "params": {"name": "a"}}]"#.into(),
            &UNRECOGNISED,
        ),
        // Not UTF-8.
        (
            b"{\"method\": \"tools/call\", \"params\": {\"name\": \"a\xff\"}}".into(),
            &UNRECOGNISED,
        ),
        // A method naming no tool keeps its key-less reading whatever `params` holds.
        (
            r#"{"method": "resources/read", "params": [1]}"#.into(),
            &KEYLESS,
        ),
        (
            r#"{"method": "resources/read", "params": 1e400}"#.into(),
            &KEYLESS,
        ),
    ] {
        assert_eq!(
            &read_endpoint(MOUNT, &McpReading::read(&body)),
            named,
            "{:.60}",
            String::from_utf8_lossy(&body)
        );
    }
}
