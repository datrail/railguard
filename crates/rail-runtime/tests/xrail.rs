use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rail_runtime::{OpaqueTicket, Secret, Unavailable, XRailHeader, XRailHeaders};

fn ticket() -> Arc<OpaqueTicket> {
    Arc::new(OpaqueTicket {
        token: Secret::new("a.b.c".to_owned()),
        expires_at: SystemTime::now() + Duration::from_secs(900),
    })
}

const SENT: [&str; 8] = [
    "X-Rail",
    "x-rail-other",
    "x-railing",
    "accept",
    "X-RAIL-STATUS",
    "x_rail",
    "X_Rail_Status",
    "foo_bar",
];

#[test]
fn without_rail_center_every_agent_xrail_header_is_removed_and_none_set() {
    assert_eq!(
        XRailHeaders::of(None, SENT),
        XRailHeaders {
            set: None,
            remove: vec![
                "foo_bar".to_owned(),
                "x-rail".to_owned(),
                "x-rail-other".to_owned(),
                "x-rail-status".to_owned(),
                "x_rail".to_owned(),
                "x_rail_status".to_owned(),
            ],
        }
    );
}

#[test]
fn the_ticket_replaces_the_agents_x_rail() {
    assert_eq!(
        XRailHeaders::of(Some(Ok(ticket())), SENT),
        XRailHeaders {
            set: Some(XRailHeader::Ticket(Secret::new("a.b.c".to_owned()))),
            remove: vec![
                "foo_bar".to_owned(),
                "x-rail-other".to_owned(),
                "x-rail-status".to_owned(),
                "x_rail".to_owned(),
                "x_rail_status".to_owned(),
            ],
        }
    );
}

#[test]
fn the_reason_replaces_the_agents_x_rail_status() {
    assert_eq!(
        XRailHeaders::of(Some(Err(Unavailable::Expired)), SENT),
        XRailHeaders {
            set: Some(XRailHeader::Status("expired")),
            remove: vec![
                "foo_bar".to_owned(),
                "x-rail".to_owned(),
                "x-rail-other".to_owned(),
                "x_rail".to_owned(),
                "x_rail_status".to_owned(),
            ],
        }
    );
}

#[test]
fn nothing_else_the_agent_did_not_send_is_removed() {
    let headers = XRailHeaders::of(Some(Err(Unavailable::NotFound)), ["accept"]);

    assert_eq!(headers.remove, vec!["x-rail".to_owned()]);
}

#[test]
fn the_gateways_headers_are_removed_when_the_host_reports_none() {
    let none = || std::iter::empty::<&str>();

    let ticket = XRailHeaders::of(Some(Ok(ticket())), none());
    assert_eq!(ticket.set.as_ref().map(XRailHeader::name), Some("x-rail"));
    assert_eq!(ticket.remove, vec!["x-rail-status".to_owned()]);

    let status = XRailHeaders::of(Some(Err(Unavailable::Expired)), none());
    assert_eq!(status.set, Some(XRailHeader::Status("expired")));
    assert_eq!(status.remove, vec!["x-rail".to_owned()]);

    assert_eq!(
        XRailHeaders::of(None, none()).remove,
        vec!["x-rail".to_owned(), "x-rail-status".to_owned()]
    );
}

#[test]
fn the_ticket_never_debugs() {
    let headers = XRailHeaders::of(Some(Ok(ticket())), SENT);

    assert!(!format!("{headers:?}").contains("a.b.c"));
}

#[test]
fn the_status_reason_debugs() {
    let headers = XRailHeaders::of(Some(Err(Unavailable::Expired)), SENT);

    assert!(format!("{headers:?}").contains("expired"));
}

#[test]
fn every_name_with_an_underscore_is_removed() {
    let headers = XRailHeaders::of(None, ["x_rail", "X_Rail_Status", "foo_bar", "x-railing"]);

    assert_eq!(
        headers.remove,
        vec![
            "foo_bar".to_owned(),
            "x-rail".to_owned(),
            "x-rail-status".to_owned(),
            "x_rail".to_owned(),
            "x_rail_status".to_owned()
        ]
    );
}
