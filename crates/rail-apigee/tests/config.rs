use rail_apigee::Config;
use rail_runtime::Level;

// Each setting logged at INFO, as (name, value).
type Entries = Vec<(&'static str, String)>;

fn read(pairs: &[(&str, &str)]) -> Result<(Config, Entries), Vec<String>> {
    let (config, startup) = rail_test_support::read(pairs, Config::read)?;
    let entries = startup
        .entries
        .into_iter()
        .filter(|entry| entry.level == Level::Info)
        .map(|entry| (entry.name, entry.value))
        .collect();
    Ok((config, entries))
}

#[test]
fn unset_the_port_and_limit_take_their_defaults_and_rail_center_needs_its_settings() {
    let (config, entries) = read(&[("RAIL_DISABLED", "true")]).unwrap();

    assert_eq!(config.port, 8080);
    assert_eq!(config.max_message_bytes, 16 * 1024 * 1024);
    assert_eq!(config.max_body_bytes, 15 * 1024 * 1024);
    assert!(config.rail_center.is_none());
    assert_eq!(
        entries,
        [
            ("RAIL_LOG_FORMAT", "json".to_owned()),
            ("RAIL_LOG_LEVEL", "info".to_owned()),
            ("RAIL_CALLOUT_PORT", "8080".to_owned()),
            ("RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB", "16".to_owned()),
            (
                "RAIL_MAX_BODY_MB",
                "15 (4193280 bytes in force: Apigee's answer limit, on the whole request)"
                    .to_owned()
            ),
        ]
    );

    let enabled = read(&[
        ("RAIL_CENTER_URL", "https://rail-center.example"),
        ("RAIL_GATEWAY_SLUG", "gw-1"),
        ("RAIL_CALLOUT_PORT", " 9090 "),
        ("RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB", "1"),
        ("RAIL_MAX_BODY_MB", "2"),
    ])
    .unwrap();
    // Below Apigee's answer limit, so described as it is.
    assert!(enabled.1.contains(&("RAIL_MAX_BODY_MB", "2".to_owned())));
    let enabled = enabled.0;
    assert_eq!(
        (
            enabled.port,
            enabled.max_message_bytes,
            enabled.max_body_bytes
        ),
        (9090, 1024 * 1024, 2 * 1024 * 1024)
    );
    let (_, bundle) = enabled.rail_center.unwrap();
    assert_eq!(bundle.gateway_slug, "gw-1");
    assert!(read(&[("RAIL_CENTER_URL", "https://rail-center.example")]).is_err());
}

#[test]
fn a_port_or_limit_out_of_range_is_refused_and_the_largest_limit_taken() {
    let cases = [
        ("RAIL_CALLOUT_PORT", "0"),
        ("RAIL_CALLOUT_PORT", "65536"),
        ("RAIL_CALLOUT_PORT", "80a"),
        ("RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB", "0"),
        ("RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB", "101"),
        ("RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB", "1.5"),
        ("RAIL_MAX_BODY_MB", "0"),
        ("RAIL_MAX_BODY_MB", "101"),
    ];
    for (name, value) in cases {
        let errors = read(&[("RAIL_DISABLED", "true"), (name, value)]).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with(name), "{errors:?}");
    }
    let (largest, _) = read(&[
        ("RAIL_DISABLED", "true"),
        ("RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB", "100"),
    ])
    .unwrap();
    assert_eq!(largest.max_message_bytes, 100 * 1024 * 1024);
}

#[test]
fn the_retired_port_is_refused_alongside_the_other_refusals() {
    let errors = read(&[("RAIL_DISABLED", "maybe"), ("RAIL_GATEWAY_PORT", "8080")]).unwrap_err();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors.iter().any(|e| e.starts_with("RAIL_GATEWAY_PORT")),
        "{errors:?}"
    );
}

#[test]
fn the_retired_names_are_refused_each_naming_the_new_one() {
    let cases = [
        (
            "RAIL_GATEWAY_PORT",
            "8080",
            "RAIL_CALLOUT_PORT sets the port",
        ),
        (
            "RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB",
            "16",
            "RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB sets the limit",
        ),
    ];
    for (name, value, instead) in cases {
        let errors = read(&[("RAIL_DISABLED", "true"), (name, value)]).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with(name), "{errors:?}");
        assert!(errors[0].contains(instead), "{errors:?}");
    }
}
