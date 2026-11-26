use rail_model::authority::split_authority;

#[test]
fn test_reads_a_host_one_way() {
    for (authority, host, port) in [
        ("mcp.example.com", "mcp.example.com", None),
        ("MCP.Example.COM", "mcp.example.com", None),
        ("mcp.example.com.", "mcp.example.com", None),
        ("mcp.example.com.:8443", "mcp.example.com", Some(8443)),
        ("mcp.example.com:65535", "mcp.example.com", Some(65535)),
        ("my_host-1", "my_host-1", None),
        ("10.0.0.7:80", "10.0.0.7", Some(80)),
        ("[2001:DB8::1]", "2001:db8::1", None),
        ("[2001:db8::1]:8443", "2001:db8::1", Some(8443)),
    ] {
        assert_eq!(
            split_authority(authority),
            Ok((host.to_owned(), port)),
            "{authority}"
        );
    }
}
