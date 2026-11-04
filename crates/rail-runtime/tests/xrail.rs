use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rail_runtime::{OpaqueTicket, Secret, Unavailable, XRailHeaders};

fn ticket() -> Arc<OpaqueTicket> {
    Arc::new(OpaqueTicket {
        token: Secret::new("a.b.c".to_owned()),
        expires_at: SystemTime::now() + Duration::from_secs(900),
    })
}

fn set(name: &'static str, value: &str) -> Option<(&'static str, Secret)> {
    Some((name, Secret::new(value.to_owned())))
}

const SENT: [&str; 5] = [
    "X-Rail",
    "x-rail-other",
    "x-railing",
    "accept",
    "X-RAIL-STATUS",
];

#[test]
fn without_rail_center_every_agent_xrail_header_is_removed_and_none_set() {
    assert_eq!(
        XRailHeaders::of(None, SENT),
        XRailHeaders {
            set: None,
            remove: vec![
                "x-rail".to_owned(),
                "x-rail-other".to_owned(),
                "x-rail-status".to_owned()
            ],
        }
    );
}

#[test]
fn the_ticket_replaces_the_agents_x_rail() {
    assert_eq!(
        XRailHeaders::of(Some(Ok(ticket())), SENT),
        XRailHeaders {
            set: set("x-rail", "a.b.c"),
            remove: vec!["x-rail-other".to_owned(), "x-rail-status".to_owned()],
        }
    );
}

#[test]
fn the_reason_replaces_the_agents_x_rail_status() {
    assert_eq!(
        XRailHeaders::of(Some(Err(Unavailable::Expired)), SENT),
        XRailHeaders {
            set: set("x-rail-status", "expired"),
            remove: vec!["x-rail".to_owned(), "x-rail-other".to_owned()],
        }
    );
}

#[test]
fn nothing_the_agent_did_not_send_is_removed() {
    let headers = XRailHeaders::of(Some(Err(Unavailable::NotFound)), ["accept"]);

    assert!(headers.remove.is_empty());
}

#[test]
fn the_ticket_never_debugs() {
    let headers = XRailHeaders::of(Some(Ok(ticket())), SENT);

    assert!(!format!("{headers:?}").contains("a.b.c"));
}
