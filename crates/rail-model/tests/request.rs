use rail_model::body::{BodyRead, Unread};
use rail_model::request::{ObservedRequest, read_path};

#[test]
fn the_path_is_the_uri_without_its_query_percent_decoded() {
    let cases = [
        ("/mcp", "/mcp"),
        ("/mcp?session=1", "/mcp"),
        ("/a%20b/%2Fc?x=%20", "/a b//c"),
        // An invalid escape is kept, invalid UTF-8 replaced.
        ("/a%zz", "/a%zz"),
        ("/a%ff", "/a\u{fffd}"),
        ("", "/"),
        ("?only=query", "/"),
        // Only a `?` ends it, and a `CONNECT`'s authority is read as a path.
        ("/a#b?c", "/a#b"),
        ("mcp.example:443", "mcp.example:443"),
        // A byte the host replaced is kept beside the escapes decoded.
        ("/\u{fffd}%41", "/\u{fffd}A"),
    ];
    for (uri, path) in cases {
        assert_eq!(read_path(uri), path, "{uri:?}");
    }
}

// Its path and headers can carry credentials.
#[test]
fn test_debugs_a_request_without_its_path_or_headers() {
    let header = vec![(b"authorization".to_vec(), b"Bearer sk-1".to_vec())];
    let body = BodyRead::Unread(Unread::NotSent);
    let request = ObservedRequest::new(
        b"a.test".to_vec(),
        b"GET".to_vec(),
        b"/sk-2".to_vec(),
        header,
        body,
        0,
    );
    let debug = format!("{request:?}");
    assert!(
        debug.contains("a.test") && !debug.contains("sk-"),
        "{debug}"
    );
}
