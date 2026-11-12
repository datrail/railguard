use rail_runtime::logging::LogFormat;
use rail_runtime::{
    ConfigErrors, Settings, Source, Startup, Variables, read_log_format, read_log_level,
};
use tracing::Level;

use crate::common;

fn read(value: Option<&str>) -> Result<Level, Vec<String>> {
    let pairs: Vec<_> = value
        .map(|value| ("RAIL_LOG_LEVEL", value))
        .into_iter()
        .collect();
    common::read(&pairs, read_log_level)
}

#[test]
fn the_log_level_is_info_by_default_and_case_folded() {
    assert_eq!(read(None).unwrap(), Level::INFO);
    assert_eq!(read(Some(" ")).unwrap(), Level::INFO);
    assert_eq!(read(Some("DEBUG")).unwrap(), Level::DEBUG);
    assert_eq!(read(Some("error")).unwrap(), Level::ERROR);
}

#[test]
fn other_level_names_and_the_retired_per_host_settings_are_refused() {
    for value in ["WARNING", "CRITICAL", "off"] {
        let errors = read(Some(value)).unwrap_err();
        assert_eq!(
            errors,
            [format!(
                "RAIL_LOG_LEVEL must be error, warn, info, debug or trace, got: {value}"
            )]
        );
    }
    for name in ["RAIL_GATEWAY_LOG_LEVEL", "RAIL_PROXY_LOG_LEVEL"] {
        let errors = common::read(&[(name, "info")], read_log_level).unwrap_err();
        assert_eq!(
            errors,
            [format!(
                "{name} is no longer read; RAIL_LOG_LEVEL sets the log level"
            )]
        );
    }
}

fn read_format(value: &str) -> Result<LogFormat, Vec<String>> {
    common::read(&[("RAIL_LOG_FORMAT", value)], read_log_format)
}

#[test]
fn the_log_format_is_json_by_default_and_case_folded() {
    assert_eq!(common::read(&[], read_log_format).unwrap(), LogFormat::Json);
    assert_eq!(read_format(" ").unwrap(), LogFormat::Json);
    assert_eq!(read_format("JSON").unwrap(), LogFormat::Json);
    assert_eq!(read_format(" gcp ").unwrap(), LogFormat::Gcp);
    assert_eq!(read_format("text").unwrap(), LogFormat::Text);
}

// Cloud Run sets one of these; an explicit value still wins.
#[test]
fn on_cloud_run_the_log_format_is_detected_as_gcp() {
    for platform in ["K_SERVICE", "CLOUD_RUN_JOB"] {
        let (format, startup) = read_startup(&[(platform, "callout")]).unwrap();
        assert_eq!(format, LogFormat::Gcp, "{platform}");
        assert_eq!(
            startup
                .entries
                .iter()
                .map(|entry| (entry.name, entry.value.as_str(), entry.source))
                .collect::<Vec<_>>(),
            [("RAIL_LOG_FORMAT", "gcp", Source::Detected)],
            "{platform}"
        );
        assert!(startup.unread.is_empty());

        let (format, startup) =
            read_startup(&[(platform, "callout"), ("RAIL_LOG_FORMAT", "text")]).unwrap();
        assert_eq!(format, LogFormat::Text);
        assert_eq!(startup.entries[0].source, Source::Env);
    }
    // Blank is unset, as for any variable.
    assert_eq!(
        read_startup(&[("K_SERVICE", " ")]).unwrap().0,
        LogFormat::Json
    );
}

fn read_startup(pairs: &[(&str, &str)]) -> Result<(LogFormat, Startup), ConfigErrors> {
    let variables: Variables = pairs.iter().copied().collect();
    let mut settings = Settings::new(&variables);
    let format = read_log_format(&mut settings);
    settings.finish("RAIL_", format)
}

#[test]
fn another_log_format_is_refused() {
    for value in ["logfmt", "gcp-json", "stackdriver"] {
        assert_eq!(
            read_format(value).unwrap_err(),
            [format!(
                "RAIL_LOG_FORMAT must be text, json or gcp, got: {value}"
            )]
        );
    }
}
