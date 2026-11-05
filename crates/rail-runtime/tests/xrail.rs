use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rail_runtime::{
    OpaqueTicket, ProtectedHosts, Route, Secret, Unavailable, XRailHeader, XRailHeaders,
};

use crate::common::protect;

fn ticket() -> Arc<OpaqueTicket> {
    Arc::new(OpaqueTicket {
        token: Secret::new("a.b.c".to_owned()),
        expires_at: SystemTime::now() + Duration::from_secs(900),
    })
}

fn mcp() -> ProtectedHosts {
    protect("mcp.example.com")
}

fn route(tls: bool, authority: &str) -> Option<Route> {
    Some(Route {
        tls,
        authority: authority.to_owned(),
    })
}

fn names(remove: &[&str]) -> Vec<String> {
    remove.iter().map(|&name| name.to_owned()).collect()
}

const SENT: [&str; 9] = [
    "X-Rail",
    "x-rail-other",
    "x-railing",
    "accept",
    "X-RAIL-STATUS",
    "X-Rail-Upstream",
    "x_rail",
    "X_Rail_Status",
    "foo_bar",
];

#[test]
fn without_rail_center_a_protected_host_is_routed_with_no_xrail_header() {
    assert_eq!(
        XRailHeaders::of(None, "mcp.example.com", &mcp(), SENT),
        XRailHeaders {
            set: None,
            route: route(true, "mcp.example.com"),
            remove: names(&[
                "foo_bar",
                "x-rail",
                "x-rail-other",
                "x-rail-status",
                "x_rail",
                "x_rail_status",
            ]),
        }
    );
}

#[test]
fn the_ticket_replaces_the_agents_x_rail() {
    assert_eq!(
        XRailHeaders::of(Some(Ok(ticket())), "mcp.example.com", &mcp(), SENT),
        XRailHeaders {
            set: Some(XRailHeader::Ticket(Secret::new("a.b.c".to_owned()))),
            route: route(true, "mcp.example.com"),
            remove: names(&[
                "foo_bar",
                "x-rail-other",
                "x-rail-status",
                "x_rail",
                "x_rail_status",
            ]),
        }
    );
}

#[test]
fn the_reason_replaces_the_agents_x_rail_status() {
    assert_eq!(
        XRailHeaders::of(
            Some(Err(Unavailable::Expired)),
            "mcp.example.com",
            &mcp(),
            SENT
        ),
        XRailHeaders {
            set: Some(XRailHeader::Status("expired")),
            route: route(true, "mcp.example.com"),
            remove: names(&[
                "foo_bar",
                "x-rail",
                "x-rail-other",
                "x_rail",
                "x_rail_status"
            ]),
        }
    );
}

#[test]
fn every_header_set_is_exposed_with_its_value() {
    let headers = XRailHeaders::of(Some(Ok(ticket())), "mcp.example.com:9443", &mcp(), SENT);

    assert_eq!(
        headers.expose_set(),
        [
            ("x-rail", "a.b.c"),
            ("x-rail-upstream", "tls"),
            (":authority", "mcp.example.com:9443"),
        ]
    );
    assert!(
        XRailHeaders::of(Some(Ok(ticket())), "open.example.com", &mcp(), SENT)
            .expose_set()
            .is_empty()
    );
}

#[test]
fn an_unprotected_request_gets_nothing_and_loses_the_agents_xrail_headers() {
    let hosts = protect("mcp.example.com,10.0.0.7,[2001:db8::1]");
    for authority in [
        "open.example.com",
        "open.example.com:8080",
        "mcp.example.com.evil.example",
        "evil-mcp.example.com",
        "10.0.0.8",
        "[2001:db8::2]:8443",
    ] {
        for ticket in [Some(Ok(ticket())), Some(Err(Unavailable::NotFound)), None] {
            assert_eq!(
                XRailHeaders::of(ticket, authority, &hosts, SENT),
                XRailHeaders {
                    set: None,
                    route: None,
                    remove: names(&[
                        "foo_bar",
                        "x-rail",
                        "x-rail-other",
                        "x-rail-status",
                        "x-rail-upstream",
                        "x_rail",
                        "x_rail_status",
                    ]),
                },
                "{authority}"
            );
        }
    }
}

#[test]
fn an_authority_that_cannot_be_read_is_not_protected() {
    for authority in [
        "mcp.example.com:abc",
        "mcp.example.com:",
        "mcp.example.com:0",
        "[2001:db8::1",
        "user@mcp.example.com",
        "mcp.example.com/",
        "",
    ] {
        let headers = XRailHeaders::of(Some(Ok(ticket())), authority, &mcp(), ["accept"]);

        assert_eq!(headers.set, None, "{authority}");
        assert_eq!(headers.route, None, "{authority}");
    }
}

#[test]
fn where_a_protected_request_goes() {
    // HTTPS unless the entry says `http://`; the entry's port, else the agent's, where `:80` is none.
    for (entry, authority, tls, rewritten) in [
        (
            "mcp.example.com",
            "mcp.example.com",
            true,
            "mcp.example.com",
        ),
        (
            "mcp.example.com",
            "mcp.example.com:80",
            true,
            "mcp.example.com",
        ),
        (
            "mcp.example.com",
            "mcp.example.com:9443",
            true,
            "mcp.example.com:9443",
        ),
        (
            "mcp.example.com:8443",
            "mcp.example.com",
            true,
            "mcp.example.com:8443",
        ),
        (
            "mcp.example.com:8443",
            "mcp.example.com:80",
            true,
            "mcp.example.com:8443",
        ),
        (
            "mcp.example.com:8443",
            "mcp.example.com:9999",
            true,
            "mcp.example.com:8443",
        ),
        (
            "http://gw.internal",
            "gw.internal:8080",
            false,
            "gw.internal:8080",
        ),
        ("http://gw.internal", "gw.internal", false, "gw.internal"),
        (
            "http://gw.internal:8080",
            "gw.internal",
            false,
            "gw.internal:8080",
        ),
    ] {
        let headers = XRailHeaders::of(None, authority, &protect(entry), ["accept"]);

        assert_eq!(headers.route, route(tls, rewritten), "{entry} {authority}");
    }
}

#[test]
fn the_host_is_matched_as_the_entry_was_read() {
    // Case, one trailing dot and IPv6 brackets don't change the host.
    for (entry, authority, rewritten) in [
        ("mcp.example.com", "MCP.Example.COM", "mcp.example.com"),
        ("mcp.example.com", "mcp.example.com.", "mcp.example.com"),
        (
            "mcp.example.com.",
            "mcp.example.com:9443",
            "mcp.example.com:9443",
        ),
        ("[2001:db8::1]", "[2001:DB8::1]:8443", "[2001:db8::1]:8443"),
        ("[2001:db8::1]:8443", "[2001:db8::1]", "[2001:db8::1]:8443"),
        ("10.0.0.7", "10.0.0.7:80", "10.0.0.7"),
    ] {
        let headers = XRailHeaders::of(None, authority, &protect(entry), ["accept"]);

        assert_eq!(headers.route, route(true, rewritten), "{entry} {authority}");
    }
}

#[test]
fn only_xrail_and_underscored_headers_are_removed_whatever_their_case() {
    let sent = [
        "Content-Type",
        "Authorization",
        ":path",
        "X-Rail",
        "X-Rail-Foo",
        "x-railway-id",
        "x-rails",
    ];

    let headers = XRailHeaders::of(None, "mcp.example.com", &mcp(), sent);

    assert_eq!(
        headers.remove,
        names(&["x-rail", "x-rail-foo", "x-rail-status"])
    );
}

#[test]
fn a_header_that_is_set_is_never_also_removed() {
    let sent = [
        "x-rail",
        "x-rail-status",
        "x-rail-upstream",
        "x-rail-foo",
        ":authority",
    ];

    let headers = XRailHeaders::of(Some(Ok(ticket())), "mcp.example.com", &mcp(), sent);

    assert_eq!(headers.remove, names(&["x-rail-foo", "x-rail-status"]));
    for (name, _) in headers.expose_set() {
        assert!(
            !headers.remove.iter().any(|removed| removed == name),
            "{name}"
        );
    }
}

#[test]
fn the_headers_envoy_and_the_gateway_read_are_removed_when_the_host_reports_none() {
    let none = || std::iter::empty::<&str>();

    let ticketed = XRailHeaders::of(Some(Ok(ticket())), "mcp.example.com", &mcp(), none());
    assert_eq!(ticketed.set.as_ref().map(XRailHeader::name), Some("x-rail"));
    assert_eq!(ticketed.remove, names(&["x-rail-status"]));

    let status = XRailHeaders::of(
        Some(Err(Unavailable::Expired)),
        "mcp.example.com",
        &mcp(),
        none(),
    );
    assert_eq!(status.set, Some(XRailHeader::Status("expired")));
    assert_eq!(status.remove, names(&["x-rail"]));

    assert_eq!(
        XRailHeaders::of(None, "mcp.example.com", &mcp(), none()).remove,
        names(&["x-rail", "x-rail-status"])
    );
    assert_eq!(
        XRailHeaders::of(Some(Ok(ticket())), "open.example.com", &mcp(), none()).remove,
        names(&["x-rail", "x-rail-status", "x-rail-upstream"])
    );
}

#[test]
fn the_ticket_never_debugs() {
    let headers = XRailHeaders::of(Some(Ok(ticket())), "mcp.example.com", &mcp(), SENT);

    assert!(!format!("{headers:?}").contains("a.b.c"));
}

#[test]
fn the_status_reason_debugs() {
    let headers = XRailHeaders::of(
        Some(Err(Unavailable::Expired)),
        "mcp.example.com",
        &mcp(),
        SENT,
    );

    assert!(format!("{headers:?}").contains("expired"));
}

#[test]
fn every_name_with_an_underscore_is_removed() {
    let headers = XRailHeaders::of(
        None,
        "open.example.com",
        &mcp(),
        ["x_rail", "X_Rail_Status", "foo_bar", "x-railing"],
    );

    assert_eq!(
        headers.remove,
        names(&[
            "foo_bar",
            "x-rail",
            "x-rail-status",
            "x-rail-upstream",
            "x_rail",
            "x_rail_status",
        ])
    );
}
