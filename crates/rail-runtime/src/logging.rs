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

use rail_policy::{escape_unquoted, sanitize_quoted};
use tracing::field::{Field, Visit};
use tracing_subscriber::field::RecordFields;
use tracing_subscriber::fmt::FormatFields;
use tracing_subscriber::fmt::format::Writer;

// Long enough to keep the root cause at the end of an error chain.
const MAX_FIELD_CHARS: usize = 1024;

/// Logs to stderr for the rest of the process, each field written by [`QuotedFields`], panics
/// included.
pub fn init() {
    tracing_subscriber::fmt()
        .fmt_fields(QuotedFields)
        .with_writer(std::io::stderr)
        .init();
    // The default hook writes a panic's message raw, outside the formatter.
    std::panic::set_hook(Box::new(log_panic));
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
        if self.result.is_err() {
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
