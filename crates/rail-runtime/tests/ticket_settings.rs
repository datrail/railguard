use std::time::Duration;

use rail_runtime::{Settings, TicketQuery, TicketSettings, Variables, read_ticket_settings};

fn read(pairs: &[(&str, &str)]) -> Result<TicketSettings, Vec<String>> {
    let variables: Variables = pairs.iter().copied().collect();
    let mut settings = Settings::new(&variables);
    let read = read_ticket_settings(&mut settings, true);
    match settings.finish() {
        Ok(()) => {
            Ok(read.expect("read_ticket_settings returns the settings when it refuses nothing"))
        }
        Err(errors) => {
            assert!(read.is_none(), "nothing is read when anything is refused");
            Err(errors.0)
        }
    }
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
    assert_eq!(ticket.limits.deadline, Duration::from_secs(10));
    assert_eq!(ticket.limits.max_bytes, 256 * 1024);
    assert_eq!(ticket.max_lifetime, Duration::from_secs(86_700));
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
            "RAIL_HOST_ID has 65 characters, past the 64 Rail Center reads",
            "RAIL_SANDBOX_NAME has 256 characters, past the 255 Rail Center reads",
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
fn not_required_judges_only_what_is_set() {
    let variables: Variables = [("RAIL_SANDBOX_NAME", "s".repeat(256))]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);
    assert!(read_ticket_settings(&mut settings, false).is_none());
    assert_eq!(
        settings.finish().unwrap_err().0,
        ["RAIL_SANDBOX_NAME has 256 characters, past the 255 Rail Center reads"]
    );

    let variables: Variables = [("RAIL_HOST_ID", "h")].into_iter().collect();
    let mut settings = Settings::new(&variables);
    assert!(read_ticket_settings(&mut settings, false).is_none());
    assert_eq!(settings.finish(), Ok(()));
}
