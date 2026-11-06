use std::time::Duration;

use rail_runtime::{BundleSettings, Level, read_bundle_settings};

use crate::common::{self, describe};

fn read(pairs: &[(&str, &str)]) -> Result<BundleSettings, Vec<String>> {
    common::read(pairs, |settings| read_bundle_settings(settings, true))
        .map(|bundle| bundle.expect("required settings are read or refused"))
}

const NAMED: [(&str, &str); 1] = [("RAIL_GATEWAY_SLUG", "edge")];

#[test]
fn the_slug_is_read_and_the_interval_defaults_to_60_s() {
    assert_eq!(
        read(&NAMED).unwrap(),
        BundleSettings {
            gateway_slug: "edge".to_owned(),
            refresh_interval: Duration::from_secs(60),
        }
    );
}

#[test]
fn the_slug_is_required() {
    assert_eq!(
        read(&[]).unwrap_err(),
        ["RAIL_GATEWAY_SLUG is required and is unset or blank"]
    );
}

#[test]
fn a_slug_past_rail_centers_bound_is_refused() {
    let slug = "s".repeat(65);

    assert_eq!(
        read(&[("RAIL_GATEWAY_SLUG", &slug)]).unwrap_err(),
        ["RAIL_GATEWAY_SLUG has 65 characters, past the 64 allowed"]
    );
    assert!(read(&[("RAIL_GATEWAY_SLUG", &"s".repeat(64))]).is_ok());
}

#[test]
fn a_slug_is_what_rail_center_accepts_as_one() {
    for taken in ["edge", "EU-west_2", "-", "_9"] {
        assert_eq!(
            read(&[("RAIL_GATEWAY_SLUG", taken)]).unwrap().gateway_slug,
            taken
        );
    }
    for (refused, quoted) in [
        ("edge 1", "edge 1"),
        ("edge/1", "edge/1"),
        ("edge.1", "edge.1"),
        ("edge?x=1", "edge?x=1"),
        ("édge", "édge"),
        ("edge\u{1b}[31m", "edge\\u{1b}[31m"),
    ] {
        assert_eq!(
            read(&[("RAIL_GATEWAY_SLUG", refused)]).unwrap_err(),
            [format!(
                "RAIL_GATEWAY_SLUG must hold only ASCII letters, digits, - and _, as Rail Center's slugs do, got: {quoted}"
            )]
        );
    }
}

#[test]
fn the_refresh_interval_is_whole_seconds_from_5_to_a_day() {
    for (taken, seconds) in [("5", 5), ("3600", 3600), (" 120 ", 120), ("86400", 86_400)] {
        let mut pairs = NAMED.to_vec();
        pairs.push(("RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS", taken));

        assert_eq!(
            read(&pairs).unwrap().refresh_interval,
            Duration::from_secs(seconds)
        );
    }
    for refused in [
        "0",
        "4",
        "-30",
        "6.0",
        "60s",
        "sixty",
        "86401",
        "18446744073709551615",
        "18446744073709551616",
    ] {
        let mut pairs = NAMED.to_vec();
        pairs.push(("RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS", refused));

        assert_eq!(
            read(&pairs).unwrap_err(),
            [format!(
                "RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS must be a whole number of seconds, from 5 to 86400, got: {refused}"
            )]
        );
    }
}

#[test]
fn a_blank_interval_is_the_default() {
    let mut pairs = NAMED.to_vec();
    pairs.push(("RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS", " "));

    assert_eq!(
        read(&pairs).unwrap().refresh_interval,
        Duration::from_secs(60)
    );
}

#[test]
fn each_setting_read_is_described_as_used() {
    let pairs = [
        ("RAIL_GATEWAY_SLUG", "edge"),
        ("RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS", " 600\n"),
    ];

    assert_eq!(
        describe(&pairs, |settings| read_bundle_settings(settings, true)),
        [
            ("RAIL_GATEWAY_SLUG", "edge".to_owned(), Level::Info),
            (
                "RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS",
                "600".to_owned(),
                Level::Info
            ),
        ]
    );
    assert_eq!(
        describe(&NAMED, |settings| read_bundle_settings(settings, true))[1],
        (
            "RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS",
            "60".to_owned(),
            Level::Info
        )
    );
}

#[test]
fn not_required_judges_only_what_is_set() {
    assert_eq!(
        common::read(
            &[("RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS", "4")],
            |settings| { read_bundle_settings(settings, false) }
        )
        .unwrap_err(),
        [
            "RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS must be a whole number of seconds, from 5 to 86400, got: 4"
        ]
    );

    let read = common::read(&[], |settings| read_bundle_settings(settings, false));
    assert!(read.unwrap().is_none());
}

#[test]
fn incomplete_describes_only_what_is_set_as_read() {
    assert_eq!(
        describe(
            &[("RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS", " 600\n")],
            |settings| read_bundle_settings(settings, false)
        ),
        [(
            "RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS",
            "600".to_owned(),
            Level::Info
        )]
    );
}
