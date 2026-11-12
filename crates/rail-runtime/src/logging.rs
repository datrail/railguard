//! The log, on stderr: the one place text is made safe to read in it.
//!
//! [`QuotedFields`] writes each field of an event or span. The message stays unquoted, escaped as
//! [`escape_unquoted`] escapes it, so it can't start a line or reach the terminal. Any other text
//! is written `name="…"`, through [`sanitize_quoted`] cut at 1024 characters, and a number or a
//! bool bare. So code logs text as a `%` or `&str` field, and never sanitizes it for the log.
//!
//! A `?` value is quoted as its `Debug` text, so a `String` logged with `?` shows Rust's own
//! escaping inside the quotes: text is logged with `%`.

use std::error::Error;
use std::fmt::{self, Write};
use std::panic::PanicHookInfo;
use std::sync::OnceLock;

use rail_policy::{escape_unquoted, sanitize, sanitize_quoted};
use tracing::field::{Field, Visit};
use tracing_subscriber::Registry;
use tracing_subscriber::field::RecordFields;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt::FormatFields;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::layer::SubscriberExt;
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

static FILTER: OnceLock<reload::Handle<Targets, Registry>> = OnceLock::new();

/// Logs to stderr for the rest of the process, each field written by [`QuotedFields`], panics
/// included; at INFO until [`set_level`].
pub fn init() {
    let (filter, handle) = reload::Layer::new(own_lines_at(tracing::Level::INFO));
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .fmt_fields(QuotedFields)
                .with_writer(std::io::stderr),
        )
        .init();
    let _ = FILTER.set(handle);
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

// A formatted message may quote what came from the wire, so only a literal one is logged.
fn log_panic(info: &PanicHookInfo<'_>) {
    let location = info.location().map(ToString::to_string);
    let location = location.as_deref();
    match info.payload().downcast_ref::<&'static str>() {
        Some(&panic_message) => tracing::error!(location, panic_message, "a thread panicked"),
        None => tracing::error!(location, "a thread panicked; its message is withheld"),
    }
}

/// The release this binary was built as, its commit, and the role it serves as: the first line a
/// host logs.
pub fn log_build(role: &str) {
    tracing::info!(version = VERSION, revision = REVISION, role, "starting");
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
        let mut text = value.to_string();
        let mut source = value.source();
        while let Some(cause) = source {
            write!(text, ": {cause}").expect("a String takes any text");
            source = cause.source();
        }
        self.write_text(field, &text);
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
