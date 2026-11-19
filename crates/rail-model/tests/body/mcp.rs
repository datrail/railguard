use rail_model::body::mcp::McpReading;

use super::keep;

fn read(method: Option<&str>, tool: Option<&str>) -> McpReading {
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
            read(Some("tools/call"), Some("a")),
        ),
        // A tool only on a `tools/call`.
        (
            r#"{"method": "resources/read", "params": {"name": "a"}}"#,
            read(Some("resources/read"), None),
        ),
        (
            r#"{"method": 1, "params": {"name": "a"}}"#,
            read(None, None),
        ),
        (
            r#"{"method": "tools/call", "params": {"name": 1}}"#,
            read(Some("tools/call"), None),
        ),
        (
            r#"{"method": "tools/call", "params": ["a"]}"#,
            read(Some("tools/call"), None),
        ),
        // An empty body (an event stream's `GET`), or JSON that isn't JSON-RPC, names nothing.
        ("", read(None, None)),
        (r#"{"jsonrpc": "2.0"}"#, read(None, None)),
        ("5", read(None, None)),
        // What these name can't be told.
        (
            r#"[{"method": "tools/call", "params": {"name": "a"}}]"#,
            McpReading::Unreadable,
        ),
        ("not json", McpReading::Unreadable),
        (" ", McpReading::Unreadable),
    ] {
        assert_eq!(McpReading::read(body.as_bytes()), reading, "{body}");
        assert_eq!(keep(body.as_bytes()).mcp(), &reading, "{body}");
    }
}
