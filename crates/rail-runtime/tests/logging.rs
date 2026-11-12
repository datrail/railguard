use std::error::Error;
use std::fmt;

use rail_runtime::logging::{log_build, log_settings};
use rail_runtime::{Entry, Level, Source, Startup};

use crate::logs::Logs;

// The one line `log` writes, after its target.
fn format_line(log: impl FnOnce()) -> String {
    let logs = Logs::default();
    {
        let _logging = logs.capture();
        log();
    }
    let text = logs.text();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 1, "{text}");
    let (_, fields) = lines[0]
        .split_once(&format!(" {}: ", module_path!()))
        .unwrap_or_else(|| panic!("{text}"));
    fields.to_owned()
}

#[test]
fn a_text_value_is_quoted_with_what_hides_escaped() {
    let cases = [
        ("a\nb", r#"m value="a\u{a}b""#),
        ("\u{1b}[31mred", r#"m value="\u{1b}[31mred""#),
        (r#"say "hi""#, r#"m value="say \"hi\"""#),
        (r"C:\dir\", r#"m value="C:\\dir\\""#),
        ("\u{202e}cba", r#"m value="\u{202e}cba""#),
        ("a\u{2028}b", r#"m value="a\u{2028}b""#),
        ("café", r#"m value="café""#),
    ];
    for (value, expected) in cases {
        assert_eq!(
            format_line(|| tracing::info!(value, "m")),
            expected,
            "{value:?}"
        );
    }
}

#[test]
fn a_value_under_1024_characters_is_kept_whole() {
    let value = format!("{}root cause", "a".repeat(1000));

    assert_eq!(
        format_line(|| tracing::info!(value = %value, "m")),
        format!("m value=\"{value}\"")
    );
}

#[test]
fn a_long_value_is_cut_inside_the_quotes() {
    let value = "a".repeat(1100);

    assert_eq!(
        format_line(|| tracing::info!(value, "m")),
        format!("m value=\"{}…\"", "a".repeat(1024))
    );
}

#[test]
fn numbers_and_bools_are_bare() {
    assert_eq!(
        format_line(|| tracing::info!(
            count = 3u64,
            delta = -2i64,
            big = 1u128 << 100,
            least = i128::MIN,
            ratio = 1.5,
            ok = true,
            "m"
        )),
        "m count=3 delta=-2 big=1267650600228229401496703205376 \
         least=-170141183460469231731687303715884105728 ratio=1.5 ok=true"
    );
}

#[test]
fn a_name_is_written_without_its_raw_prefix() {
    assert_eq!(
        format_line(|| tracing::info!(r#type = "t", "m")),
        r#"m type="t""#
    );
    // tracing strips `r#` from an identifier, but passes a quoted name on as written.
    assert_eq!(
        format_line(|| tracing::info!("r#type" = "t", "m")),
        r#"m type="t""#
    );
}

struct Hostile;

impl fmt::Display for Hostile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("shown \"as\"\nDisplay")
    }
}

impl fmt::Debug for Hostile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("shown \\as\\\nDebug")
    }
}

#[test]
fn a_display_value_is_quoted_as_its_display_text() {
    assert_eq!(
        format_line(|| tracing::info!(value = %Hostile, "m")),
        r#"m value="shown \"as\"\u{a}Display""#
    );
}

#[test]
fn a_debug_value_is_quoted_as_its_debug_text() {
    assert_eq!(
        format_line(|| tracing::info!(value = ?Hostile, "m")),
        r#"m value="shown \\as\\\u{a}Debug""#
    );
    assert_eq!(
        format_line(|| tracing::info!(value = ?Some(3), "m")),
        r#"m value="Some(3)""#
    );
    // Rust's own escaping, inside ours: text is logged with `%`.
    assert_eq!(
        format_line(|| tracing::info!(value = ?"a\nb", "m")),
        r#"m value="\"a\\nb\"""#
    );
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
fn an_error_is_quoted_with_its_sources() {
    let error = Failure("outer", Some(Box::new(Failure("in\"ner\n", None))));

    assert_eq!(
        format_line(|| tracing::info!(error = &error as &(dyn Error + 'static), "m")),
        r#"m error="outer: in\"ner\u{a}""#
    );
}

#[test]
fn a_hostile_message_is_escaped_unquoted() {
    let text = "a \"b\" \\ c\n2026-01-01T00:00:00Z  INFO fake: \u{1b}[31m";

    assert_eq!(
        format_line(|| tracing::info!("{text}")),
        r#"a \"b\" \\ c\u{a}2026-01-01T00:00:00Z  INFO fake: \u{1b}[31m"#
    );
    assert_eq!(
        format_line(|| tracing::info!(message = text)),
        r#"a \"b\" \\ c\u{a}2026-01-01T00:00:00Z  INFO fake: \u{1b}[31m"#
    );
}

#[test]
fn a_long_message_is_not_cut() {
    let text = "a".repeat(300);

    assert_eq!(format_line(|| tracing::info!("{text}")), text);
}

#[test]
fn a_span_field_is_quoted_too_when_created_and_when_recorded() {
    let logs = Logs::default();
    {
        let _logging = logs.capture();
        let span = tracing::info_span!("work", host = "a\nb", late = tracing::field::Empty);
        let _entered = span.enter();
        span.record("late", "c\u{1b}d");
        tracing::info!("m");
    }

    let text = logs.text();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(
        text.contains(&format!(
            r#" INFO work{{host="a\u{{a}}b" late="c\u{{1b}}d"}}: {}: m"#,
            module_path!()
        )),
        "{text}"
    );
}

// Each line after its timestamp: the level, target, message and fields.
#[test]
fn the_settings_are_logged_each_at_its_level_then_each_unread_name() {
    let startup = Startup {
        entries: vec![
            Entry {
                name: "RAIL_PORT",
                value: "8080".to_owned(),
                level: Level::Info,
                source: Source::Default,
            },
            Entry {
                name: "RAIL_DISABLED",
                value: "true".to_owned(),
                level: Level::Warn("nothing is fetched"),
                source: Source::Env,
            },
        ],
        unread: vec!["RAIL_TYPO".to_owned()],
    };
    let logs = Logs::default();
    {
        let _logging = logs.capture();
        log_settings(&startup);
    }

    let text = logs.text();
    let lines: Vec<_> = text
        .lines()
        .map(|line| line.split_once(' ').unwrap().1.trim_start())
        .collect();
    assert_eq!(
        lines,
        [
            r#"INFO rail_runtime::logging: setting name="RAIL_PORT" value="8080" source="default""#,
            r#"WARN rail_runtime::logging: setting name="RAIL_DISABLED" value="true" source="env" weakens="nothing is fetched""#,
            r#"WARN rail_runtime::logging: set but not read name="RAIL_TYPO""#,
        ]
    );
}

#[test]
fn the_build_is_logged_with_its_version_and_revision() {
    let logs = Logs::default();
    {
        let _logging = logs.capture();
        log_build("proxy");
    }

    let text = logs.text();
    let line = text.split_once(' ').unwrap().1.trim_start();
    // Tests build without RAIL_VERSION and RAIL_REVISION.
    assert_eq!(
        line.trim_end(),
        r#"INFO rail_runtime::logging: starting version="dev" revision="unknown" role="proxy""#
    );
}

// Run in a child process, since `init` sets the process's subscriber and panic hook.
#[test]
#[ignore = "run by init_logs_panics_through_the_formatter_and_libraries_from_warn"]
fn init_child() {
    rail_runtime::logging::init();
    log::warn!(target: "reqwest::connect", "a library's warning");
    log::info!(target: "reqwest::connect", "a library's note");
    rail_runtime::logging::set_level(tracing::Level::DEBUG);
    log::debug!(target: "reqwest::connect", "a library's detail");
    let wire = "from the wire\nERROR forged";
    let _ = std::panic::catch_unwind(|| panic!("{wire}"));
    let _ = std::panic::catch_unwind(|| panic!("a literal message"));
}

#[test]
fn init_logs_panics_through_the_formatter_and_libraries_from_warn() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "logging::init_child", "--ignored", "--nocapture"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    let file = file!();
    assert!(
        stderr.contains(&format!(
            "a thread panicked; its message is withheld location=\"{file}:"
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("a thread panicked location=\"{file}:"))
            && stderr.contains("panic_message=\"a literal message\""),
        "{stderr}"
    );
    assert!(!stderr.contains("from the wire"), "{stderr}");
    assert!(
        stderr.contains("WARN reqwest::connect: a library's warning\n"),
        "{stderr}"
    );
    assert!(!stderr.contains("a library's note"), "{stderr}");
    assert!(!stderr.contains("a library's detail"), "{stderr}");
}
