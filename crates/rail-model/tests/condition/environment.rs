use rail_model::condition::RequestLine;

// The path as received can hold a secret, which its redacted form doesn't.
#[test]
fn test_debugs_a_request_line_without_its_path() {
    let request_line = RequestLine {
        host: "mcp.example".to_string(),
        method: "POST".to_string(),
        path: "/s3cret/mcp".to_string(),
        redacted_path: "/[redacted]/mcp".to_string(),
    };
    let printed = format!("{request_line:?}");
    assert!(!printed.contains("s3cret"), "{printed}");
    assert!(printed.contains("/[redacted]/mcp"), "{printed}");
}
