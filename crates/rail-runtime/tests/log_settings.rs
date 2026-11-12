use rail_runtime::logging::LogFormat;
use rail_runtime::{read_log_format, read_log_level};
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
fn the_log_format_is_text_by_default_and_case_folded() {
    assert_eq!(common::read(&[], read_log_format).unwrap(), LogFormat::Text);
    assert_eq!(read_format(" ").unwrap(), LogFormat::Text);
    assert_eq!(read_format("JSON").unwrap(), LogFormat::Json);
    assert_eq!(read_format(" gcp ").unwrap(), LogFormat::Gcp);
    assert_eq!(read_format("text").unwrap(), LogFormat::Text);
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
