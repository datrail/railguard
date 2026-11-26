use rail_model::authority::parse_authority;

// Refused here, but read as a host by RailMon's collector, so not a vector: the vectors hold only what both accept.
#[test]
fn test_refuses_a_malformed_authority() {
    for (authority, refusal) in [
        ("mcp.example.com:", "has an empty or zero port"),
        ("mcp.example.com:0", "has an empty or zero port"),
        (
            "mcp.example.com:abc",
            "has a port that isn't a number up to 65535",
        ),
        (
            "mcp.example.com:65536",
            "has a port that isn't a number up to 65535",
        ),
        ("mcp.example.com..", "is not a host name or an IP address"),
    ] {
        assert_eq!(parse_authority(authority), Err(refusal), "{authority}");
    }
}
