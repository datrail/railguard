use rail_runtime::{
    Level, ProtectedHost, ProtectedHosts, Settings, Variables, read_protected_hosts,
};

use crate::common::{self, describe};

const FORMAT: &str = "an entry is [http(s)://]host[:port]";

fn read_with(value: Option<&str>, required: bool) -> Result<ProtectedHosts, Vec<String>> {
    let pairs: Vec<_> = value
        .map(|value| ("RAIL_PROXY_PROTECTED_HOSTS", value))
        .into_iter()
        .collect();
    common::read(&pairs, |settings| read_protected_hosts(settings, required))
}

fn read(value: &str) -> Result<ProtectedHosts, Vec<String>> {
    read_with(Some(value), true)
}

fn host(host: &str, tls: bool, port: Option<u16>) -> ProtectedHost {
    ProtectedHost {
        host: host.to_owned(),
        tls,
        port,
    }
}

#[test]
fn each_entry_form_is_read() {
    for (entry, expected) in [
        ("mcp.example.com", host("mcp.example.com", true, None)),
        (
            "mcp.example.com:8443",
            host("mcp.example.com", true, Some(8443)),
        ),
        (
            "https://mcp.example.com",
            host("mcp.example.com", true, None),
        ),
        (
            "https://mcp.example.com:9443",
            host("mcp.example.com", true, Some(9443)),
        ),
        ("http://gw.internal", host("gw.internal", false, None)),
        (
            "http://gw.internal:8080",
            host("gw.internal", false, Some(8080)),
        ),
        ("HTTP://GW.Internal", host("gw.internal", false, None)),
        ("mcp.example.com.", host("mcp.example.com", true, None)),
        ("10.0.0.7", host("10.0.0.7", true, None)),
        ("[2001:db8::1]:8443", host("2001:db8::1", true, Some(8443))),
        ("[2001:DB8::1]", host("2001:db8::1", true, None)),
        ("http://[::1]", host("::1", false, None)),
        (
            "tool_server.internal",
            host("tool_server.internal", true, None),
        ),
        (
            "mcp.example.com:065535",
            host("mcp.example.com", true, Some(65535)),
        ),
    ] {
        let hosts = read(entry).unwrap();

        assert_eq!(hosts.get(&expected.host), Some(&expected), "{entry}");
        assert_eq!(hosts.hosts().count(), 1, "{entry}");
    }
}

#[test]
fn entries_are_comma_separated_and_trimmed() {
    let hosts = read(" a.example , http://b.example:8080 ").unwrap();

    let names: Vec<&str> = hosts.hosts().map(|host| host.host.as_str()).collect();
    assert_eq!(names, ["a.example", "b.example"]);
}

#[test]
fn an_entry_that_is_more_or_less_than_a_host_is_refused() {
    for (entry, says) in [
        ("https://mcp.example.com/mcp", "names more than a host"),
        ("mcp.example.com/", "names more than a host"),
        ("ftp://mcp.example.com", "other than http:// or https://"),
        ("mcp.example.com:", "empty or zero port"),
        ("mcp.example.com:0", "empty or zero port"),
        ("mcp.example.com:99999", "isn't a number up to 65535"),
        ("mcp.example.com:https", "isn't a number up to 65535"),
        ("mcp.example.com:+80", "isn't a number up to 65535"),
        ("[2001:db8::1", "not a host name or an IP address"),
        ("[2001:db8::1]8443", "not a host name or an IP address"),
        ("[10.0.0.7]", "not a host name or an IP address"),
        ("2001:db8::1", "isn't a number up to 65535"),
        ("https://", "names no host"),
        (":8443", "names no host"),
        (".", "names no host"),
        ("mcp.example.com other.example", "not a host name"),
        ("mcp..example.com", "not a host name"),
        ("mcp.example.com..", "not a host name"),
        ("mcp.exämple.com", "not a host name"),
        ("mcp%2eexample.com", "not a host name"),
    ] {
        let errors = read(entry).unwrap_err();

        assert_eq!(errors.len(), 1, "{entry}: {errors:#?}");
        let error = &errors[0];
        assert!(
            error.starts_with("RAIL_PROXY_PROTECTED_HOSTS has an entry that "),
            "{error}"
        );
        assert!(error.contains(says), "{entry}: {error}");
        assert!(
            error.ends_with(&format!("; {FORMAT}, got: {entry}")),
            "{error}"
        );
    }
}

#[test]
fn a_refused_entry_is_quoted_sanitized() {
    assert_eq!(
        read("mcp.example.com\u{202e}moc").unwrap_err(),
        [format!(
            "RAIL_PROXY_PROTECTED_HOSTS has an entry that is not a host name or an IP address; \
             {FORMAT}, got: mcp.example.com\\u{{202e}}moc"
        )]
    );
}

#[test]
fn a_credential_in_an_entry_is_refused_and_never_quoted() {
    for required in [true, false] {
        for entry in [
            "user:secret@mcp.example.com",
            "https://token@mcp.example.com:8443",
            "user:secret@[2001:db8::1",
            "mcp.example.com/p@th",
        ] {
            let errors = read_with(Some(&format!("a.example,{entry}")), required).unwrap_err();

            assert_eq!(
                errors,
                [format!(
                    "RAIL_PROXY_PROTECTED_HOSTS has a credential in entry 2 of 2, which isn't \
                     quoted; {FORMAT}"
                )]
            );
        }
    }
}

#[test]
fn a_query_or_fragment_in_an_entry_is_refused_and_never_quoted() {
    for entry in [
        "mcp.example.com?token=s3cret",
        "mcp.example.com?",
        "https://mcp.example.com/#s3cret",
        "mcp.example.com#",
    ] {
        let errors = read(&format!("a.example,{entry}")).unwrap_err();

        assert_eq!(
            errors,
            [format!(
                "RAIL_PROXY_PROTECTED_HOSTS has a query or fragment in entry 2 of 2, which \
                 isn't quoted; {FORMAT}"
            )]
        );
        assert!(!errors[0].contains("s3cret"), "{errors:#?}");
    }
}

#[test]
fn an_empty_entry_is_refused() {
    for (value, position) in [
        ("a.example,,b.example", "2 of 3"),
        ("a.example,", "2 of 2"),
        (",a.example", "1 of 2"),
        ("a.example, \t ,b.example", "2 of 3"),
    ] {
        assert_eq!(
            read(value).unwrap_err(),
            [format!(
                "RAIL_PROXY_PROTECTED_HOSTS has an empty entry, entry {position}; {FORMAT}"
            )],
            "{value}"
        );
    }
}

#[test]
fn entries_are_counted_with_the_empty_ones() {
    assert_eq!(
        read("a.example,,user:pw@b.example").unwrap_err(),
        [
            format!("RAIL_PROXY_PROTECTED_HOSTS has an empty entry, entry 2 of 3; {FORMAT}"),
            format!(
                "RAIL_PROXY_PROTECTED_HOSTS has a credential in entry 3 of 3, which isn't \
                 quoted; {FORMAT}"
            ),
        ]
    );
}

#[test]
fn every_refused_entry_is_listed_at_once() {
    let errors =
        read("ftp://a.example, b.example:0, user:secret@c.example, d.example").unwrap_err();

    assert_eq!(errors.len(), 3, "{errors:#?}");
}

#[test]
fn one_host_listed_twice_is_refused() {
    for (value, host) in [
        ("mcp.example.com,mcp.example.com", "mcp.example.com"),
        (
            "mcp.example.com,https://MCP.example.com:8443",
            "mcp.example.com",
        ),
        ("http://gw.internal,gw.internal.", "gw.internal"),
        ("[2001:db8::1],[2001:DB8::1]:8443", "2001:db8::1"),
    ] {
        assert_eq!(
            read(value).unwrap_err(),
            [format!(
                "RAIL_PROXY_PROTECTED_HOSTS lists {host} more than once; one entry per host"
            )],
            "{value}"
        );
    }
}

#[test]
fn required_it_must_name_a_host() {
    assert_eq!(
        read_with(None, true).unwrap_err(),
        [
            "RAIL_PROXY_PROTECTED_HOSTS is required because the proxy attaches the agent's \
             identity, so it must name where to send it, and is unset or blank"
        ]
    );
    for value in ["", "   "] {
        assert_eq!(read_with(Some(value), true).unwrap_err().len(), 1);
    }
    assert_eq!(
        read(" , ").unwrap_err(),
        [
            format!("RAIL_PROXY_PROTECTED_HOSTS has an empty entry, entry 1 of 2; {FORMAT}"),
            format!("RAIL_PROXY_PROTECTED_HOSTS has an empty entry, entry 2 of 2; {FORMAT}"),
        ]
    );
}

#[test]
fn each_host_is_described_and_one_over_http_as_a_warning() {
    let value = "mcp.example.com:8443,API.example.com,http://gw.internal,[::1]:9000";

    assert_eq!(
        describe(&[("RAIL_PROXY_PROTECTED_HOSTS", value)], |settings| {
            read_protected_hosts(settings, true)
        }),
        [
            (
                "RAIL_PROXY_PROTECTED_HOSTS",
                "[::1]:9000 (HTTPS)".to_owned(),
                Level::Info
            ),
            (
                "RAIL_PROXY_PROTECTED_HOSTS",
                "api.example.com, at the agent's port (HTTPS)".to_owned(),
                Level::Info
            ),
            (
                "RAIL_PROXY_PROTECTED_HOSTS",
                "gw.internal, at the agent's port (HTTP)".to_owned(),
                Level::Warn("the ticket crosses in the clear")
            ),
            (
                "RAIL_PROXY_PROTECTED_HOSTS",
                "mcp.example.com:8443 (HTTPS)".to_owned(),
                Level::Info
            ),
        ]
    );
}

#[test]
fn no_host_is_described_as_none() {
    assert_eq!(
        describe(&[], |settings| read_protected_hosts(settings, false)),
        [("RAIL_PROXY_PROTECTED_HOSTS", "none".to_owned(), Level::Info)]
    );
}

#[test]
fn not_required_judges_only_what_is_set() {
    assert_eq!(read_with(None, false), Ok(ProtectedHosts::default()));
    assert_eq!(read_with(Some("   "), false), Ok(ProtectedHosts::default()));
    assert_eq!(read_with(Some(" , "), false).unwrap_err().len(), 2);
    assert!(
        read_with(Some("mcp.example.com"), false)
            .unwrap()
            .get("mcp.example.com")
            .is_some()
    );
    assert_eq!(
        read_with(Some("ftp://mcp.example.com"), false)
            .unwrap_err()
            .len(),
        1
    );
}

#[test]
fn a_refused_entry_beside_a_good_one_is_an_err() {
    for value in [
        "a.example.com,",
        "a.example.com,ftp://b.example.com",
        "a.example.com,u@b.example.com",
        "a.example.com,A.example.com",
    ] {
        let variables: Variables = [("RAIL_PROXY_PROTECTED_HOSTS", value)]
            .into_iter()
            .collect();
        let mut settings = Settings::new(&variables);

        assert!(
            read_protected_hosts(&mut settings, true).is_err(),
            "{value}"
        );
    }
}
