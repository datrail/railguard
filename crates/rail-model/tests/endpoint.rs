use rail_model::endpoint::ResolvedKey;

#[test]
fn test_reads_a_resolved_key() {
    let key: ResolvedKey = "/mcp#tools/call#a#b".parse().unwrap();
    assert_eq!(key.as_str(), "/mcp#tools/call#a#b");

    for text in [
        "/mcp#tools/call",
        "/mcp#tools/call#",
        "#tools/call#a",
        "/mcp##a",
        "",
    ] {
        assert!(text.parse::<ResolvedKey>().is_err(), "{text:?}");
    }
}
