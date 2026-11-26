use rail_model::request_line::RequestLine;

fn read(authority: &str, method: &str, path: &str) -> RequestLine {
    RequestLine::read(authority.as_bytes(), method.as_bytes(), path.as_bytes())
}

// The path as received can hold a secret, which its redacted form doesn't.
#[test]
fn test_debugs_a_request_line_without_its_path() {
    let line = read("mcp.example.com", "POST", "/s3cret-s3cret-s3cret/mcp");

    let printed = format!("{line:?}");
    assert!(!printed.contains("s3cret"), "{printed}");
    assert!(printed.contains("/[redacted]/mcp"), "{printed}");
}

#[test]
fn test_cuts_the_redacted_path_to_its_first_segments() {
    for (path, segments, prefix) in [("/team/fr/mcp", 2, "/team/fr"), ("/team/fr/mcp", 0, "/")] {
        let line = read("mcp.example.com", "POST", path);

        assert_eq!(line.path_prefix(segments), prefix, "{path} {segments}");
    }
}
