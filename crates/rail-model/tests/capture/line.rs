use rail_model::capture::{ReportedMethod, Unmatchable, path_prefix, reported_method, unmatchable};
use rail_model::request_line::RequestLine;

fn read(method: &str, path: &str) -> RequestLine {
    RequestLine::read(b"mcp.example.com", method.as_bytes(), path.as_bytes())
}

#[test]
fn test_reports_a_standard_method_as_sent_and_any_other_as_other() {
    for method in ["GET", "PATCH"] {
        let line = read(method, "/");

        assert_eq!(reported_method(&line), ReportedMethod::Standard(method));
        assert_eq!(reported_method(&line).as_str(), method);
    }
    let line = read("post", "/");

    assert_eq!(reported_method(&line), ReportedMethod::Other);
    assert_eq!(reported_method(&line).as_str(), "[other]");
}

#[test]
fn test_tells_why_a_path_matches_nothing() {
    for (path, why) in [
        ("/mcp/", None),
        ("/a.b/..c/.d", None),
        ("/mcp?x=[1]%2F/../a//b", None),
        ("/[redacted]/mcp", Some(Unmatchable::Bracket)),
        ("/a]", Some(Unmatchable::Bracket)),
        ("/%5Bredacted%5D/mcp", Some(Unmatchable::Bracket)),
        ("/%5bredacted/mcp", Some(Unmatchable::Bracket)),
        ("/a%5d", Some(Unmatchable::Bracket)),
        ("/a%2Fb", Some(Unmatchable::EncodedSlash)),
        ("/a]%2Fb/..//c", Some(Unmatchable::Bracket)),
        ("/a%2Fb/..//c", Some(Unmatchable::EncodedSlash)),
        ("/../a//c", Some(Unmatchable::DotSegment)),
        ("/./mcp", Some(Unmatchable::DotSegment)),
        ("/a/../mcp", Some(Unmatchable::DotSegment)),
        ("/mcp//", Some(Unmatchable::EmptySegment)),
    ] {
        assert_eq!(unmatchable(&read("POST", path)), why, "{path}");
    }
}

#[test]
fn test_a_path_that_isnt_utf8_matches_nothing() {
    let line = RequestLine::read(b"mcp.example.com", b"POST", b"/mcp/\xff");

    assert_eq!(unmatchable(&line), Some(Unmatchable::NotUtf8));
}

#[test]
fn test_cuts_the_redacted_path_to_its_first_segments() {
    for (path, segments, prefix) in [
        ("/finretail-production-mcp/mcp", 1, "/[redacted]"),
        ("/team/fr/mcp", 2, "/team/fr"),
        ("/mcp/", 3, "/mcp"),
        ("/team/fr/mcp", 0, "/"),
    ] {
        let line = read("POST", path);

        assert_eq!(path_prefix(&line, segments), prefix, "{path} {segments}");
    }
}
