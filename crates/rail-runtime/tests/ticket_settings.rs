use crate::common;
use std::time::Duration;

use rail_runtime::{Level, TicketQuery, TicketSettings, read_ticket_settings};

use crate::common::describe;

fn read(pairs: &[(&str, &str)]) -> Result<TicketSettings, Vec<String>> {
    common::read(pairs, |settings| read_ticket_settings(settings, true))
        .map(|ticket| ticket.expect("required settings are read or refused"))
}

const NAMED: [(&str, &str); 2] = [
    ("RAIL_HOST_ID", "host-1"),
    ("RAIL_SANDBOX_NAME", "sandbox a"),
];

#[test]
fn the_host_and_sandbox_are_read_and_the_rest_defaults() {
    let ticket = read(&NAMED).unwrap();

    assert_eq!(
        ticket.query,
        TicketQuery {
            host_id: "host-1".to_owned(),
            sandbox_name: "sandbox a".to_owned(),
        }
    );
    assert_eq!(ticket.refresh_interval, Duration::from_secs(3600));
}

#[test]
fn the_host_and_sandbox_are_both_required() {
    assert_eq!(
        read(&[]).unwrap_err(),
        [
            "RAIL_HOST_ID is required and is unset or blank",
            "RAIL_SANDBOX_NAME is required and is unset or blank",
        ]
    );
}

#[test]
fn a_name_past_rail_centers_bound_is_refused() {
    let host = "h".repeat(65);
    let sandbox = "é".repeat(256);

    let errors = read(&[("RAIL_HOST_ID", &host), ("RAIL_SANDBOX_NAME", &sandbox)]).unwrap_err();

    assert_eq!(
        errors,
        [
            "RAIL_HOST_ID has 65 characters, past the 64 allowed",
            "RAIL_SANDBOX_NAME has 256 characters, past the 255 allowed",
        ]
    );
    let at_the_bound = ("h".repeat(64), "é".repeat(255));
    assert!(
        read(&[
            ("RAIL_HOST_ID", &at_the_bound.0),
            ("RAIL_SANDBOX_NAME", &at_the_bound.1)
        ])
        .is_ok()
    );
}

#[test]
fn the_refresh_interval_is_whole_seconds_from_5() {
    let mut pairs = NAMED.to_vec();
    pairs.push(("RAIL_TICKET_REFRESH_SECONDS", "600"));
    assert_eq!(
        read(&pairs).unwrap().refresh_interval,
        Duration::from_secs(600)
    );

    for refused in ["0", "4", "-1", "1.5", "1h"] {
        let mut pairs = NAMED.to_vec();
        pairs.push(("RAIL_TICKET_REFRESH_SECONDS", refused));

        assert_eq!(
            read(&pairs).unwrap_err(),
            [format!(
                "RAIL_TICKET_REFRESH_SECONDS must be a whole number of seconds, at least 5, got: {refused}"
            )]
        );
    }
}

#[test]
fn the_refresh_interval_is_described_as_used() {
    assert_eq!(
        describe(&NAMED, |settings| read_ticket_settings(settings, true)),
        [
            ("RAIL_HOST_ID", "host-1".to_owned(), Level::Info),
            ("RAIL_SANDBOX_NAME", "sandbox a".to_owned(), Level::Info),
            (
                "RAIL_TICKET_REFRESH_SECONDS",
                "3600".to_owned(),
                Level::Info
            ),
        ]
    );

    let mut pairs = NAMED.to_vec();
    pairs.push(("RAIL_TICKET_REFRESH_SECONDS", " 600\n"));
    assert_eq!(
        describe(&pairs, |settings| read_ticket_settings(settings, true))[2],
        ("RAIL_TICKET_REFRESH_SECONDS", "600".to_owned(), Level::Info)
    );
}

#[test]
fn not_required_judges_only_what_is_set() {
    let long = "s".repeat(256);
    assert_eq!(
        common::read(&[("RAIL_SANDBOX_NAME", &long)], |settings| {
            read_ticket_settings(settings, false)
        })
        .unwrap_err(),
        ["RAIL_SANDBOX_NAME has 256 characters, past the 255 allowed"]
    );

    let read = common::read(&[("RAIL_HOST_ID", "h")], |settings| {
        read_ticket_settings(settings, false)
    });
    assert!(read.unwrap().is_none());
}

#[test]
fn incomplete_describes_only_what_is_set_as_read() {
    assert_eq!(
        describe(
            &[
                ("RAIL_TICKET_REFRESH_SECONDS", " 600\n"),
                ("RAIL_HOST_ID", "h")
            ],
            |settings| read_ticket_settings(settings, false)
        ),
        [
            ("RAIL_HOST_ID", "h".to_owned(), Level::Info),
            ("RAIL_TICKET_REFRESH_SECONDS", "600".to_owned(), Level::Info),
        ]
    );
}
