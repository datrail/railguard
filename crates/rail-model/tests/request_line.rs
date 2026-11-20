use rail_model::request_line::{HostReading, RequestLine};

fn read(authority: &str, method: &str, path: &str) -> RequestLine {
    RequestLine::read(authority.as_bytes(), method.as_bytes(), path.as_bytes())
}

#[test]
fn test_reads_the_host_as_split_authority_spells_it() {
    let longest = "a".repeat(253);
    let past = "a".repeat(254);
    for (authority, host, reading) in [
        (
            "MCP.Example.com.:8443",
            "mcp.example.com",
            HostReading::Read,
        ),
        (longest.as_str(), longest.as_str(), HostReading::Read),
        (&format!("{longest}."), longest.as_str(), HostReading::Read),
        (
            "user@mcp.example.com",
            "[unparseable]",
            HostReading::Refused,
        ),
        (past.as_str(), "[unparseable]", HostReading::PastLongest),
    ] {
        let line = read(authority, "POST", "/mcp");

        assert_eq!(line.host(), host, "{authority}");
        assert_eq!(line.host_reading(), reading, "{authority}");
    }
}

#[test]
fn test_reads_an_authority_that_isnt_utf8_as_refused() {
    let line = RequestLine::read(b"mcp.ex\xffample.com", b"POST", b"/mcp");

    assert_eq!(line.host(), "[unparseable]");
    assert_eq!(line.host_reading(), HostReading::Refused);
}

#[test]
fn test_reads_the_path_up_to_its_query_or_fragment_never_decoded() {
    for (path, read_as, redacted) in [
        ("/mcp/?key=sk-live-abc12345", "/mcp/", "/mcp"),
        ("/mcp#sk-live-abc12345?x", "/mcp", "/mcp"),
        ("/m%63p", "/m%63p", "/m%63p"),
        ("?x", "", "/"),
    ] {
        let line = read("mcp.example.com", "POST", path);

        assert_eq!(line.path(), read_as, "{path}");
        assert_eq!(line.redacted_path(), redacted, "{path}");
        assert!(!line.path_lossy(), "{path}");
    }
}

#[test]
fn test_reads_a_path_that_isnt_utf8_lossily() {
    let line = RequestLine::read(b"mcp.example.com", b"POST", b"/mcp/\xff");

    assert_eq!(line.path(), "/mcp/\u{fffd}");
    assert!(line.path_lossy());
}

#[test]
fn test_reads_a_connects_path_as_the_root() {
    let line = read("[2001:db8::1]:443", "CONNECT", "[2001:db8::1]:443");

    assert_eq!(line.path(), "/");
    assert_eq!(line.redacted_path(), "/");
}

#[test]
fn test_reads_the_path_of_a_method_other_than_connect_as_sent() {
    let line = read("mcp.example.com:443", "connect", "mcp.example.com:443");

    assert_eq!(line.method(), "connect");
    assert_eq!(line.path(), "mcp.example.com:443");
}

// The path as received can hold a secret, which its redacted form doesn't.
#[test]
fn test_debugs_a_request_line_without_its_path() {
    let line = read("mcp.example.com", "POST", "/s3cret-s3cret-s3cret/mcp");

    let printed = format!("{line:?}");
    assert!(!printed.contains("s3cret"), "{printed}");
    assert!(printed.contains("/[redacted]/mcp"), "{printed}");
}
