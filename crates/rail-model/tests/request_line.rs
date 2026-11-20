use rail_model::request_line::{HostReading, ReportedMethod, RequestLine, Unmatchable};

fn read(authority: &str, method: &str, path: &str) -> RequestLine {
    RequestLine::read(authority.as_bytes(), method.as_bytes(), path.as_bytes())
}

#[test]
fn test_reads_the_host_as_split_authority_spells_it() {
    let longest = "a".repeat(253);
    for (authority, host) in [
        ("MCP.Example.com.:8443", "mcp.example.com"),
        (longest.as_str(), longest.as_str()),
        (&format!("{longest}."), longest.as_str()),
    ] {
        let line = read(authority, "POST", "/mcp");

        assert_eq!(line.host(), host, "{authority}");
        assert_eq!(
            line.host_reading(),
            &HostReading::Read(host.to_owned()),
            "{authority}"
        );
    }
}

#[test]
fn test_reads_a_host_it_cant_report_as_unparseable() {
    let past = "a".repeat(254);
    for (authority, reading) in [
        (&b"user@mcp.example.com"[..], HostReading::Refused),
        (b"mcp.ex\xffample.com", HostReading::Refused),
        (past.as_bytes(), HostReading::PastLongest),
    ] {
        let line = RequestLine::read(authority, b"POST", b"/mcp");

        assert_eq!(line.host(), "[unparseable]", "{authority:?}");
        assert_eq!(line.host_reading(), &reading, "{authority:?}");
    }
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
    }
}

#[test]
fn test_reads_a_path_that_isnt_utf8_lossily_matching_nothing() {
    // A bracket too, so not UTF-8 is the reason told first.
    let line = RequestLine::read(b"mcp.example.com", b"POST", b"/[mcp]/\xff");

    assert_eq!(line.path(), "/[mcp]/\u{fffd}");
    assert_eq!(line.unmatchable(), Some(Unmatchable::NotUtf8));
}

#[test]
fn test_reads_a_path_utf8_up_to_its_query_whatever_follows() {
    for path in [&b"/mcp?\xff"[..], "/mcp/\u{fffd}".as_bytes()] {
        let line = RequestLine::read(b"mcp.example.com", b"POST", path);

        assert_eq!(line.unmatchable(), None, "{path:?}");
    }
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

#[test]
fn test_reports_a_standard_method_as_sent_and_any_other_as_other() {
    for method in ["GET", "PATCH"] {
        let line = read("mcp.example.com", method, "/");

        assert_eq!(line.reported_method(), ReportedMethod::Standard(method));
        assert_eq!(line.reported_method().as_str(), method);
    }
    let line = read("mcp.example.com", "post", "/");

    assert_eq!(line.reported_method(), ReportedMethod::Other);
    assert_eq!(line.reported_method().as_str(), "[other]");
}

#[test]
fn test_tells_why_a_path_matches_nothing() {
    for (path, why) in [
        ("/mcp/", None),
        ("/a.b/..c/.d", None),
        ("/mcp?x=[1]%2F/../a//b", None),
        ("/[redacted]/mcp", Some(Unmatchable::Bracket)),
        ("/a[", Some(Unmatchable::Bracket)),
        ("/a]", Some(Unmatchable::Bracket)),
        ("/%5Bredacted%5D/mcp", Some(Unmatchable::Bracket)),
        ("/%5bredacted/mcp", Some(Unmatchable::Bracket)),
        ("/a%5d", Some(Unmatchable::Bracket)),
        ("/a%2Fb", Some(Unmatchable::EncodedSlash)),
        ("/a]%2Fb/..//c", Some(Unmatchable::Bracket)),
        ("/a%2Fb/..//c", Some(Unmatchable::EncodedSlash)),
        ("/../a//c", Some(Unmatchable::DotSegment)),
        ("/./mcp", Some(Unmatchable::DotSegment)),
        ("/mcp//", Some(Unmatchable::EmptySegment)),
    ] {
        assert_eq!(
            read("mcp.example.com", "POST", path).unmatchable(),
            why,
            "{path}"
        );
    }
}

#[test]
fn test_cuts_the_redacted_path_to_its_first_segments() {
    for (path, segments, prefix) in [
        ("/finretail-production-mcp/mcp", 1, "/[redacted]"),
        ("/team/fr/mcp", 2, "/team/fr"),
        ("/mcp/", 3, "/mcp"),
        ("/team/fr/mcp", 0, "/"),
    ] {
        let line = read("mcp.example.com", "POST", path);

        assert_eq!(line.path_prefix(segments), prefix, "{path} {segments}");
    }
}
