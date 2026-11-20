//! How capture reads a call: whether its request line can match an entry, and what a record
//! reports of it.

mod line;

pub use line::{ReportedMethod, Unmatchable, path_prefix, reported_method, unmatchable};
