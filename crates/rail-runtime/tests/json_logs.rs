use std::error::Error;
use std::fmt;
use std::process::{Command, Output};

use rail_runtime::ConfigErrors;
use serde_json::{Value, json};

use crate::logs::Logs;

// The one line `log` writes, as it was written.
fn write_line(severity: bool, log: impl FnOnce()) -> String {
    let logs = Logs::default();
    {
        let _logging = logs.capture_json(severity);
        log();
    }
    let text = logs.text();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 1, "{text}");
    lines[0].to_owned()
}

// That line's members, but the timestamp, checked to be RFC 3339 in UTC.
fn read_line(severity: bool, log: impl FnOnce()) -> Value {
    let line = write_line(severity, log);
    let mut value: Value =
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}: {line}"));
    let timestamp = value["timestamp"]
        .as_str()
        .unwrap_or_else(|| panic!("{line}"));
    assert!(
        time::OffsetDateTime::parse(timestamp, &time::format_description::well_known::Rfc3339)
            .is_ok_and(|instant| instant.offset().is_utc()),
        "{line}"
    );
    value.as_object_mut().unwrap().remove("timestamp");
    value
}

#[test]
fn a_line_is_its_timestamp_level_target_and_message_then_each_field_in_order() {
    let line = write_line(false, || tracing::info!(port = 8080, host = "h", "serving"));

    assert!(line.starts_with(r#"{"timestamp":""#), "{line}");
    let (_, rest) = line
        .split_once(r#"","level""#)
        .unwrap_or_else(|| panic!("{line}"));
    assert_eq!(
        rest,
        format!(
            r#":"INFO","target":"{}","message":"serving","port":8080,"host":"h"}}"#,
            module_path!()
        )
    );
}

#[test]
fn json_names_the_level_as_tracing_does() {
    let levels = [
        (read_line(false, || tracing::error!("m")), "ERROR"),
        (read_line(false, || tracing::warn!("m")), "WARN"),
        (read_line(false, || tracing::info!("m")), "INFO"),
        (read_line(false, || tracing::debug!("m")), "DEBUG"),
        (read_line(false, || tracing::trace!("m")), "TRACE"),
    ];
    for (line, level) in levels {
        assert_eq!(line["level"], level, "{line}");
        assert!(line.get("severity").is_none(), "{line}");
    }
}

#[test]
fn gcp_names_the_level_as_cloud_logging_does_in_severity() {
    let levels = [
        (read_line(true, || tracing::error!("m")), "ERROR"),
        (read_line(true, || tracing::warn!("m")), "WARNING"),
        (read_line(true, || tracing::info!("m")), "INFO"),
        (read_line(true, || tracing::debug!("m")), "DEBUG"),
        (read_line(true, || tracing::trace!("m")), "DEBUG"),
    ];
    for (line, severity) in levels {
        assert_eq!(line["severity"], severity, "{line}");
        assert!(line.get("level").is_none(), "{line}");
    }
}

#[test]
fn a_number_or_a_bool_is_bare_and_anything_else_text() {
    let log = || {
        tracing::info!(
            signed = -3_i64,
            unsigned = 7_u64,
            wide = u128::MAX,
            ratio = 0.5,
            nan = f64::NAN,
            flag = true,
            shown = %"a",
            debugged = ?"b",
            "m"
        );
    };
    // Past what a JSON reader's f64 holds, so read here as written.
    assert!(
        write_line(false, log).contains(&format!(r#","wide":{},"#, u128::MAX)),
        "{}",
        write_line(false, log)
    );
    let line = read_line(false, log);

    assert_eq!(line["signed"], json!(-3));
    assert_eq!(line["unsigned"], json!(7));
    assert_eq!(line["ratio"], json!(0.5));
    assert_eq!(line["nan"], Value::Null);
    assert_eq!(line["flag"], json!(true));
    assert_eq!(line["shown"], json!("a"));
    // As in text, `?` shows Rust's own escaping.
    assert_eq!(line["debugged"], json!("\"b\""));
}

#[test]
fn text_reads_back_with_what_hides_escaped_as_in_the_text_log() {
    let cases = [
        ("a\nb", r"a\u{a}b"),
        ("\u{1b}[31mred", r"\u{1b}[31mred"),
        ("\u{9b}31m", r"\u{9b}31m"),
        (r#"say "hi""#, r#"say "hi""#),
        (r"C:\dir\", r"C:\dir\"),
        ("\u{202e}cba", r"\u{202e}cba"),
        ("a\u{2028}b", r"a\u{2028}b"),
        ("café", "café"),
    ];
    for (value, expected) in cases {
        let line = read_line(false, || tracing::info!(value, "{value}"));
        assert_eq!(line["value"], expected, "{value:?}");
        assert_eq!(line["message"], expected, "{value:?}");
    }
}

#[test]
fn a_field_is_cut_at_1024_characters_and_the_message_never() {
    let whole = "a".repeat(1024);
    let long = "a".repeat(1100);

    let line = read_line(
        false,
        || tracing::info!(whole = %whole, long = %long, "{long}"),
    );

    assert_eq!(line["whole"], whole.as_str());
    assert_eq!(line["long"], format!("{whole}…"));
    assert_eq!(line["message"], long.as_str());
}

#[test]
fn a_field_named_as_a_key_of_the_line_is_prefixed() {
    let line = read_line(true, || {
        tracing::info!(
            timestamp = 1,
            level = 2,
            severity = 3,
            target = 4,
            r#type = 5,
            "m"
        );
    });

    assert_eq!(line["severity"], "INFO");
    assert_eq!(line["target"], module_path!());
    for (name, value) in [
        ("field.timestamp", 1),
        ("field.level", 2),
        ("field.severity", 3),
        ("field.target", 4),
        ("type", 5),
    ] {
        assert_eq!(line[name], json!(value), "{line}");
    }
}

#[derive(Debug)]
struct Failure(&'static str, Option<Box<Failure>>);

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for Failure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.1
            .as_deref()
            .map(|cause| cause as &(dyn Error + 'static))
    }
}

#[test]
fn an_error_is_written_with_its_sources() {
    let error = Failure("outer", Some(Box::new(Failure("in\"ner\n", None))));

    let line = read_line(false, || {
        tracing::info!(error = &error as &(dyn Error + 'static), "m")
    });

    assert_eq!(line["error"], r#"outer: in"ner\u{a}"#);
}

#[test]
fn an_event_with_no_message_has_an_empty_one() {
    let line = read_line(false, || tracing::info!(port = 1));

    assert_eq!(line["message"], "");
}

// Run in a child process, since `init` sets the process's subscriber and panic hook.
#[test]
#[ignore = "run by init_reads_the_format_from_the_environment"]
fn init_child() {
    rail_runtime::logging::init();
    rail_runtime::logging::log_build("proxy");
    log::warn!(target: "reqwest::connect", "a library's warning");
    let _ = std::panic::catch_unwind(|| panic!("a literal message"));
}

fn run_child(test: &str, format: Option<&str>) -> Output {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", test, "--ignored", "--nocapture"]);
    command.env_remove("RAIL_LOG_FORMAT");
    if let Some(format) = format {
        command.env("RAIL_LOG_FORMAT", format);
    }
    command.output().unwrap()
}

// The lines the child logged, each a JSON object; the test harness's own lines aren't.
fn read_json_lines(stderr: &str) -> Vec<Value> {
    stderr
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap_or_else(|error| panic!("{error}: {line}")))
        .collect()
}

#[test]
fn init_reads_the_format_from_the_environment() {
    let output = run_child("json_logs::init_child", Some(" GCP "));

    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    let lines = read_json_lines(&stderr);
    assert_eq!(lines.len(), 3, "{stderr}");
    assert_eq!(lines[0]["message"], "starting", "{stderr}");
    assert_eq!(lines[0]["severity"], "INFO", "{stderr}");
    assert_eq!(lines[0]["version"], "dev", "{stderr}");
    // A `log` record names its own target, without tracing-log's fields; keys read back sorted.
    assert_eq!(lines[1]["target"], "reqwest::connect", "{stderr}");
    assert_eq!(lines[1]["severity"], "WARNING", "{stderr}");
    assert_eq!(
        lines[1].as_object().unwrap().keys().collect::<Vec<_>>(),
        ["message", "severity", "target", "timestamp"],
        "{stderr}"
    );
    assert_eq!(lines[2]["message"], "a thread panicked", "{stderr}");
    assert_eq!(lines[2]["panic_message"], "a literal message", "{stderr}");
    assert_eq!(lines[2]["severity"], "ERROR", "{stderr}");
}

#[test]
fn init_logs_text_with_no_format_or_one_it_refuses() {
    for format in [None, Some("text"), Some("logfmt")] {
        let output = run_child("json_logs::init_child", format);

        assert!(output.status.success(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(read_json_lines(&stderr).is_empty(), "{format:?}: {stderr}");
        assert!(
            stderr.contains(r#"INFO rail_runtime::logging: starting version="dev""#),
            "{format:?}: {stderr}"
        );
    }
}

#[test]
#[ignore = "run by refusals_are_an_error_line_each_in_json"]
fn refusals_child() {
    rail_runtime::logging::init();
    ConfigErrors(vec![
        "RAIL_A must be a number, got: x".to_owned(),
        "RAIL_B is required".to_owned(),
    ])
    .exit();
}

#[test]
fn refusals_are_an_error_line_each_in_json() {
    let output = run_child("json_logs::refusals_child", Some("json"));

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("Invalid configuration detected"),
        "{stderr}"
    );
    let lines = read_json_lines(&stderr);
    let refusals: Vec<_> = lines
        .iter()
        .map(|line| {
            (
                line["level"].as_str(),
                line["message"].as_str(),
                line["refusal"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        refusals,
        [
            (
                Some("ERROR"),
                Some("invalid configuration"),
                Some("RAIL_A must be a number, got: x")
            ),
            (
                Some("ERROR"),
                Some("invalid configuration"),
                Some("RAIL_B is required")
            ),
        ],
        "{stderr}"
    );
}

#[test]
fn refusals_are_one_block_in_text() {
    let output = run_child("json_logs::refusals_child", None);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(
            "Invalid configuration detected:\n  RAIL_A must be a number, got: x\n  RAIL_B is required\n"
        ),
        "{stderr}"
    );
}
