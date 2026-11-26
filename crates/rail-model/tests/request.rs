use rail_model::request::read_path;

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
    ];
    for (uri, path) in cases {
        assert_eq!(read_path(uri), path, "{uri:?}");
    }
}
