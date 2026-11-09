use rail_apigee::Config;
use rail_runtime::{Level, Settings, Variables};

// Each setting logged at INFO, as (name, value).
type Entries = Vec<(&'static str, String)>;

fn read(pairs: &[(&str, &str)]) -> Result<(Config, Entries), Vec<String>> {
    let variables: Variables = pairs.iter().copied().collect();
    let mut settings = Settings::new(&variables);
    let read = Config::read(&mut settings);
    let (config, startup) = settings.finish("RAIL_", read).map_err(|errors| errors.0)?;
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
    assert!(config.rail_center.is_none());
    assert_eq!(
        entries,
        [
            ("RAIL_GATEWAY_PORT", "8080".to_owned()),
            ("RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB", "16".to_owned()),
        ]
    );

    let enabled = read(&[
        ("RAIL_CENTER_URL", "https://rail-center.example"),
        ("RAIL_GATEWAY_SLUG", "gw-1"),
        ("RAIL_GATEWAY_PORT", " 9090 "),
        ("RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB", "1"),
    ])
    .unwrap()
    .0;
    assert_eq!(
        (enabled.port, enabled.max_message_bytes),
        (9090, 1024 * 1024)
    );
    let (_, bundle) = enabled.rail_center.unwrap();
    assert_eq!(bundle.gateway_slug, "gw-1");
    assert!(read(&[("RAIL_CENTER_URL", "https://rail-center.example")]).is_err());
}

#[test]
fn a_port_or_limit_out_of_range_is_refused() {
    let cases = [
        ("RAIL_GATEWAY_PORT", "0"),
        ("RAIL_GATEWAY_PORT", "65536"),
        ("RAIL_GATEWAY_PORT", "80a"),
        ("RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB", "0"),
        ("RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB", "17"),
        ("RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB", "1.5"),
    ];
    for (name, value) in cases {
        let errors = read(&[("RAIL_DISABLED", "true"), (name, value)]).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with(name), "{errors:?}");
    }
}
