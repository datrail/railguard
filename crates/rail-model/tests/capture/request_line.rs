use rail_model::capture::request_line::{CallLine, HostReading, ReportedMethod, Unmatchable};

fn read(authority: &str, method: &str, path: &str) -> CallLine {
    CallLine::read(authority.as_bytes(), method.as_bytes(), path.as_bytes())
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
        ("[2001:DB8::1]:8080", "2001:db8::1", HostReading::Read),
        (longest.as_str(), longest.as_str(), HostReading::Read),
        (&format!("{longest}."), longest.as_str(), HostReading::Read),
        (
            "user@mcp.example.com",
            "[unparseable]",
            HostReading::Refused,
        ),
        ("mcp.exämple.com", "[unparseable]", HostReading::Refused),
        ("", "[unparseable]", HostReading::Refused),
        (past.as_str(), "[unparseable]", HostReading::PastLongest),
    ] {
        let line = read(authority, "POST", "/mcp");

        assert_eq!(line.request_line().host, host, "{authority}");
        assert_eq!(line.host(), reading, "{authority}");
    }
}

#[test]
fn test_reads_an_authority_that_isnt_utf8_as_refused() {
    let line = CallLine::read(b"mcp.ex\xffample.com", b"POST", b"/mcp");

    assert_eq!(line.request_line().host, "[unparseable]");
    assert_eq!(line.host(), HostReading::Refused);
}

#[test]
fn test_reports_a_standard_method_as_sent_and_any_other_as_other() {
    for method in [
        "GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH",
    ] {
        let line = read("mcp.example.com", method, "/");

        assert_eq!(line.reported_method(), ReportedMethod::Standard(method));
        assert_eq!(line.reported_method().as_str(), method);
    }
    for method in ["post", "PROPFIND", ""] {
        let line = read("mcp.example.com", method, "/");

        assert_eq!(line.request_line().method, method);
        assert_eq!(line.reported_method(), ReportedMethod::Other, "{method}");
        assert_eq!(line.reported_method().as_str(), "[other]");
    }
}

#[test]
fn test_reads_the_path_up_to_its_query_or_fragment_never_decoded() {
    for (path, read_as, redacted) in [
        ("/mcp", "/mcp", "/mcp"),
        ("/mcp/?key=sk-live-abc12345", "/mcp/", "/mcp"),
        ("/mcp#sk-live-abc12345?x", "/mcp", "/mcp"),
        ("/m%63p", "/m%63p", "/m%63p"),
        (
            "/finretail-production-mcp/mcp",
            "/finretail-production-mcp/mcp",
            "/[redacted]/mcp",
        ),
        ("/", "/", "/"),
        ("?x", "", "/"),
    ] {
        let line = read("mcp.example.com", "POST", path);

        assert_eq!(line.request_line().path, read_as, "{path}");
        assert_eq!(line.request_line().redacted_path, redacted, "{path}");
    }
}

#[test]
fn test_reads_a_connects_path_as_the_root() {
    let line = read("mcp.example.com:443", "CONNECT", "mcp.example.com:443");

    assert_eq!(line.request_line().path, "/");
    assert_eq!(line.request_line().redacted_path, "/");
    assert_eq!(line.unmatchable(), None);
}

#[test]
fn test_tells_why_a_path_matches_nothing() {
    for (path, why) in [
        ("/mcp", None),
        ("/mcp/", None),
        ("/", None),
        ("", None),
        ("/a.b/..c/.d", None),
        ("/mcp?x=[1]", None),
        ("/mcp?x=a%2Fb", None),
        ("/mcp?x=/../a//b", None),
        ("/[redacted]/mcp", Some(Unmatchable::Bracket)),
        ("/a]", Some(Unmatchable::Bracket)),
        ("/%5Bredacted%5D/mcp", Some(Unmatchable::Bracket)),
        ("/%5bredacted/mcp", Some(Unmatchable::Bracket)),
        ("/a%5d", Some(Unmatchable::Bracket)),
        ("/a%2Fb", Some(Unmatchable::EncodedSlash)),
        ("/a%2fb", Some(Unmatchable::EncodedSlash)),
        ("/./mcp", Some(Unmatchable::DotSegment)),
        ("/a/../mcp", Some(Unmatchable::DotSegment)),
        ("/mcp/.", Some(Unmatchable::DotSegment)),
        ("/a//mcp", Some(Unmatchable::EmptySegment)),
        ("//mcp", Some(Unmatchable::EmptySegment)),
        ("/mcp//", Some(Unmatchable::EmptySegment)),
    ] {
        let line = read("mcp.example.com", "POST", path);

        assert_eq!(line.unmatchable(), why, "{path}");
    }
}

#[test]
fn test_reads_a_path_that_isnt_utf8_lossily_matching_nothing() {
    let line = CallLine::read(b"mcp.example.com", b"POST", b"/mcp/\xff");

    assert_eq!(line.request_line().path, "/mcp/\u{fffd}");
    assert_eq!(line.unmatchable(), Some(Unmatchable::NotUtf8));
}

#[test]
fn test_cuts_the_redacted_path_to_its_first_segments() {
    for (path, segments, prefix) in [
        (
            "/services/T000/B000/XXXXXXXXXXXXXXXXXXXXXXXX",
            1,
            "/services",
        ),
        ("/finretail-production-mcp/mcp", 1, "/[redacted]"),
        ("/team/fr/mcp", 2, "/team/fr"),
        ("/mcp/", 3, "/mcp"),
        ("/", 1, "/"),
    ] {
        let line = read("mcp.example.com", "POST", path);

        assert_eq!(line.path_prefix(segments), prefix, "{path} {segments}");
    }
}
