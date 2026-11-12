//! The log, on stderr: the one place text is made safe to read in it.
//!
//! [`QuotedFields`] writes each field of an event or span. The message stays unquoted, escaped as
//! [`escape_unquoted`] escapes it, so it can't start a line or reach the terminal. Any other text
//! is written `name="…"`, through [`sanitize_quoted`] cut at 1024 characters, and a number or a
//! bool bare. So code logs text as a `%` or `&str` field, and never sanitizes it for the log.
//!
//! A `?` value is quoted as its `Debug` text, so a `String` logged with `?` shows Rust's own
//! escaping inside the quotes: text is logged with `%`.
//!
//! Under `RAIL_LOG_FORMAT=json` or `gcp`, [`JsonLines`] writes each event as one JSON object
//! instead, its text escaped as [`escape_hidden`] escapes it, and cut alike.

use std::error::Error;
use std::fmt::{self, Write};
use std::panic::PanicHookInfo;
use std::sync::OnceLock;

use rail_policy::{escape_hidden, escape_unquoted, sanitize, sanitize_quoted};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_log::NormalizeEvent;
use tracing_subscriber::Registry;
use tracing_subscriber::field::RecordFields;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::{Level, Refused, Settings, Startup};

// Long enough to keep the root cause at the end of an error chain.
const MAX_FIELD_CHARS: usize = 1024;

// The image build sets both; a build outside it is `dev`, at no known commit.
const VERSION: &str = match option_env!("RAIL_VERSION") {
    Some(version) => version,
    None => "dev",
};
const REVISION: &str = match option_env!("RAIL_REVISION") {
    Some(revision) => revision,
    None => "unknown",
};

const LOG_LEVEL: &str = "RAIL_LOG_LEVEL";
const LOG_LEVELS: [tracing::Level; 5] = [
    tracing::Level::ERROR,
    tracing::Level::WARN,
    tracing::Level::INFO,
    tracing::Level::DEBUG,
    tracing::Level::TRACE,
];
// The crates whose lines `RAIL_LOG_LEVEL` sets; the libraries' stay at WARN, since below it
// they log the wire itself.
const OWN_TARGETS: [&str; 5] = [
    "rail_apigee",
    "rail_envoy",
    "rail_grpc_host",
    "rail_policy",
    "rail_runtime",
];

const LOG_FORMAT: &str = "RAIL_LOG_FORMAT";
// A JSON line's own keys: a field of the same name is written `field.<name>`.
const RESERVED_KEYS: [&str; 4] = ["timestamp", "level", "severity", "target"];

static FILTER: OnceLock<reload::Handle<Targets, Registry>> = OnceLock::new();
static FORMAT: OnceLock<LogFormat> = OnceLock::new();

/// How each line is written: `RAIL_LOG_FORMAT`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// The message, then `name=value` fields, by [`QuotedFields`].
    #[default]
    Text,
    /// One JSON object per line, by [`JsonLines`], its level as `level`.
    Json,
    /// As `Json`, its level as Cloud Logging's `severity`.
    Gcp,
}

impl LogFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            LogFormat::Text => "text",
            LogFormat::Json => "json",
            LogFormat::Gcp => "gcp",
        }
    }
}

/// Logs to stderr for the rest of the process, in the format `RAIL_LOG_FORMAT` names, panics
/// included; at INFO until [`set_level`].
pub fn init() {
    // Read here, before the settings, so the first line is already in it; text if it's refused.
    let format = std::env::var(LOG_FORMAT)
        .ok()
        .and_then(|text| parse_log_format(text.trim()).ok())
        .unwrap_or_default();
    let (filter, handle) = reload::Layer::new(own_lines_at(tracing::Level::INFO));
    let text = (format == LogFormat::Text).then(|| {
        tracing_subscriber::fmt::layer()
            .fmt_fields(QuotedFields)
            .with_writer(std::io::stderr)
    });
    let json = (format != LogFormat::Text).then(|| {
        tracing_subscriber::fmt::layer()
            .event_format(JsonLines {
                severity: format == LogFormat::Gcp,
            })
            .with_writer(std::io::stderr)
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(text)
        .with(json)
        .init();
    let _ = FILTER.set(handle);
    let _ = FORMAT.set(format);
    // The default hook writes a panic's message raw, outside the formatter.
    std::panic::set_hook(Box::new(log_panic));
}

/// Logs this workspace's own lines at `level` from now on.
pub fn set_level(level: tracing::Level) {
    if let Some(handle) = FILTER.get() {
        let _ = handle.reload(own_lines_at(level));
    }
}

fn own_lines_at(level: tracing::Level) -> Targets {
    OWN_TARGETS.into_iter().fold(
        Targets::new().with_default(LevelFilter::WARN),
        |targets, target| targets.with_target(target, level),
    )
}

/// `RAIL_LOG_LEVEL`, INFO by default. Refuses the retired names for it, one per host.
pub fn read_log_level(settings: &mut Settings) -> Result<tracing::Level, Refused> {
    let instead = format!("{LOG_LEVEL} sets the log level");
    let retired = [
        settings.get("RAIL_GATEWAY_LOG_LEVEL").retired(&instead),
        settings.get("RAIL_PROXY_LOG_LEVEL").retired(&instead),
    ];
    let level = settings
        .get(LOG_LEVEL)
        .parsed(parse_log_level)
        .map(|level| level.unwrap_or(tracing::Level::INFO));
    if let Ok(level) = &level {
        settings.describe(LOG_LEVEL, level.as_str().to_ascii_lowercase());
    }
    for refused in retired {
        refused?;
    }
    level
}

fn parse_log_level(text: &str) -> Result<tracing::Level, String> {
    LOG_LEVELS
        .into_iter()
        .find(|level| level.as_str().eq_ignore_ascii_case(text))
        .ok_or_else(|| {
            format!(
                "must be error, warn, info, debug or trace, got: {}",
                sanitize(text)
            )
        })
}

/// `RAIL_LOG_FORMAT`, `text` by default. [`init`] has already read it; this judges it and logs it.
pub fn read_log_format(settings: &mut Settings) -> Result<LogFormat, Refused> {
    let format = settings
        .get(LOG_FORMAT)
        .parsed(parse_log_format)
        .map(Option::unwrap_or_default);
    if let Ok(format) = &format {
        settings.describe(LOG_FORMAT, format.as_str());
    }
    format
}

fn parse_log_format(text: &str) -> Result<LogFormat, String> {
    [LogFormat::Text, LogFormat::Json, LogFormat::Gcp]
        .into_iter()
        .find(|format| format.as_str().eq_ignore_ascii_case(text))
        .ok_or_else(|| format!("must be text, json or gcp, got: {}", sanitize(text)))
}

// Text keeps its block; a JSON reader wants each refusal as an error line of its own.
pub(crate) fn log_refusals(refusals: &[String]) -> bool {
    if FORMAT.get().is_none_or(|format| *format == LogFormat::Text) {
        return false;
    }
    for refusal in refusals {
        tracing::error!(refusal = %refusal, "invalid configuration");
    }
    true
}

// A formatted message may quote what came from the wire, so only a literal one is logged.
fn log_panic(info: &PanicHookInfo<'_>) {
    let location = info.location().map(ToString::to_string);
    let location = location.as_deref();
    match info.payload().downcast_ref::<&'static str>() {
        Some(&panic_message) => tracing::error!(location, panic_message, "a thread panicked"),
        None => tracing::error!(location, "a thread panicked; its message is withheld"),
    }
}

/// The release this binary was built as, and its commit: the first line a host logs.
pub fn log_build() {
    tracing::info!(version = VERSION, revision = REVISION, "starting");
}

// Each setting as used, then each RAIL_ variable nothing read, likely a typo; never its value.
pub fn log_settings(startup: &Startup) {
    for entry in &startup.entries {
        let source = entry.source.as_str();
        match entry.level {
            Level::Info => {
                tracing::info!(name = %entry.name, value = %entry.value, source = %source, "setting");
            }
            Level::Warn(weakens) => {
                tracing::warn!(name = %entry.name, value = %entry.value, source = %source, weakens, "setting");
            }
        }
    }
    for name in &startup.unread {
        tracing::warn!(name = %name, "set but not read");
    }
}

/// The field formatter [`init`] installs.
#[derive(Clone, Copy, Debug, Default)]
pub struct QuotedFields;

impl<'writer> FormatFields<'writer> for QuotedFields {
    fn format_fields<R: RecordFields>(&self, writer: Writer<'writer>, fields: R) -> fmt::Result {
        let mut visitor = Visitor {
            writer,
            first: true,
            result: Ok(()),
        };
        fields.record(&mut visitor);
        visitor.result
    }
}

struct Visitor<'writer> {
    writer: Writer<'writer>,
    first: bool,
    result: fmt::Result,
}

impl Visitor<'_> {
    fn write_text(&mut self, field: &Field, text: &str) {
        if field.name() == "message" {
            self.write_field(None, escape_unquoted(text));
        } else {
            self.write_field(Some(field), sanitize_quoted(text, MAX_FIELD_CHARS));
        }
    }

    fn write_field(&mut self, field: Option<&Field>, value: impl fmt::Display) {
        // tracing-log's fields repeat the target and name the library's source.
        let bridged = field.is_some_and(|field| field.name().starts_with("log."));
        if self.result.is_err() || bridged {
            return;
        }
        self.result = self.try_write_field(field, value);
    }

    fn try_write_field(&mut self, field: Option<&Field>, value: impl fmt::Display) -> fmt::Result {
        if !self.first {
            self.writer.write_char(' ')?;
        }
        self.first = false;
        if let Some(field) = field {
            let name = field.name();
            write!(self.writer, "{}=", name.strip_prefix("r#").unwrap_or(name))?;
        }
        write!(self.writer, "{value}")
    }
}

impl Visit for Visitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.write_text(field, value);
    }

    // `%` values come here too, their Debug being their Display.
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.write_text(field, &format!("{value:?}"));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn Error + 'static)) {
        self.write_text(field, &error_chain(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.write_field(Some(field), value);
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.write_field(Some(field), value);
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        self.write_field(Some(field), value);
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        self.write_field(Some(field), value);
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.write_field(Some(field), format_args!("{value:?}"));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.write_field(Some(field), value);
    }
}

/// The event formatter [`init`] installs for a JSON format: one object per line, `timestamp`
/// (RFC 3339, UTC), the level, `target` and `message`, then each field, a number or a bool bare.
/// No span is written: this workspace opens none, and the libraries' are filtered out below WARN.
#[derive(Clone, Copy, Debug, Default)]
pub struct JsonLines {
    /// The level as `severity`, in Cloud Logging's names (`WARNING`; TRACE as `DEBUG`), not
    /// `level`.
    pub severity: bool,
}

impl<S, N> FormatEvent<S, N> for JsonLines
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        _context: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        // tracing-log's events name the library's target only once normalized.
        let normalized = event.normalized_metadata();
        let metadata = normalized.as_ref().unwrap_or_else(|| event.metadata());
        let mut timestamp = String::new();
        SystemTime.format_time(&mut Writer::new(&mut timestamp))?;
        let mut fields = JsonFields::default();
        event.record(&mut fields);
        let (level_key, level) = self.name_level(*metadata.level());
        let mut line = String::from("{");
        write_member(&mut line, "timestamp", &json_string(&timestamp));
        write_member(&mut line, level_key, &json_string(level));
        write_member(&mut line, "target", &json_string(metadata.target()));
        let message = fields.message.unwrap_or_default();
        write_member(&mut line, "message", &json_string(&message));
        for (name, value) in &fields.members {
            write_member(&mut line, name, value);
        }
        line.push('}');
        writeln!(writer, "{line}")
    }
}

impl JsonLines {
    fn name_level(self, level: tracing::Level) -> (&'static str, &'static str) {
        if !self.severity {
            return ("level", level.as_str());
        }
        let severity = match level {
            tracing::Level::ERROR => "ERROR",
            tracing::Level::WARN => "WARNING",
            tracing::Level::INFO => "INFO",
            tracing::Level::DEBUG | tracing::Level::TRACE => "DEBUG",
        };
        ("severity", severity)
    }
}

fn write_member(line: &mut String, name: &str, encoded: &str) {
    if line.len() > 1 {
        line.push(',');
    }
    line.push_str(&json_string(name));
    line.push(':');
    line.push_str(encoded);
}

fn json_string(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

// Each value already encoded as JSON, in the order recorded.
#[derive(Default)]
struct JsonFields {
    message: Option<String>,
    members: Vec<(String, String)>,
}

impl JsonFields {
    fn push_text(&mut self, field: &Field, text: &str) {
        if field.name() == "message" {
            self.message = Some(escape_hidden(text, None).to_string());
        } else {
            let text = escape_hidden(text, Some(MAX_FIELD_CHARS)).to_string();
            self.push(field, json_string(&text));
        }
    }

    fn push(&mut self, field: &Field, encoded: String) {
        let name = field.name();
        // tracing-log's fields repeat the target and name the library's source.
        if name.starts_with("log.") {
            return;
        }
        let name = name.strip_prefix("r#").unwrap_or(name);
        let name = if RESERVED_KEYS.contains(&name) {
            format!("field.{name}")
        } else {
            name.to_owned()
        };
        self.members.push((name, encoded));
    }
}

impl Visit for JsonFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push_text(field, value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.push_text(field, &format!("{value:?}"));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn Error + 'static)) {
        self.push_text(field, &error_chain(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field, value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field, value.to_string());
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        self.push(field, value.to_string());
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        self.push(field, value.to_string());
    }

    // Not finite, it's `null`, as JSON has no such number.
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.push(field, serde_json::Value::from(value).to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field, value.to_string());
    }
}

// The error, then each cause in turn, so the root cause ends the line.
fn error_chain(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        write!(text, ": {cause}").expect("a String takes any text");
        source = cause.source();
    }
    text
}
