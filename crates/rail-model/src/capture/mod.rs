//! How capture reads a call: its request line, its path redacted as a plan's paths are.

mod call_line;
mod redaction;

pub use call_line::{CallLine, HostReading, ReportedMethod, Unmatchable};
pub use redaction::REDACTION_VERSION;
