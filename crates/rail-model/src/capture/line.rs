//! What capture makes of a call's request line: whether it can match, and what a record reports.

use crate::request_line::RequestLine;

// What a method outside the standard nine is reported as.
const OTHER_METHOD: &str = "[other]";

const STANDARD_METHODS: [&str; 9] = [
    "GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH",
];

/// Why a request path matches no entry, whatever its conditions say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unmatchable {
    /// Bytes that aren't UTF-8.
    NotUtf8,
    /// A `[` or `]`, raw or percent-encoded, so a path can't spell the redaction marker.
    Bracket,
    /// A `/` percent-encoded.
    EncodedSlash,
    /// A `.` or `..` segment.
    DotSegment,
    /// Two slashes in a row: an empty segment before another.
    EmptySegment,
}

/// The method a call record reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportedMethod {
    /// One of the nine standard methods, as sent.
    Standard(&'static str),
    /// Any other, so a custom method can't carry data: `[other]`.
    Other,
}

impl ReportedMethod {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Standard(method) => method,
            Self::Other => OTHER_METHOD,
        }
    }
}

/// Why the call's path matches no entry, if it can't. On the path as received, before redaction.
pub fn unmatchable(line: &RequestLine) -> Option<Unmatchable> {
    if line.path_lossy() {
        return Some(Unmatchable::NotUtf8);
    }
    let path = line.path();
    let encodes = |code: &[u8]| {
        path.as_bytes()
            .windows(3)
            .any(|window| window.eq_ignore_ascii_case(code))
    };
    if path.contains(['[', ']']) || encodes(b"%5b") || encodes(b"%5d") {
        return Some(Unmatchable::Bracket);
    }
    if encodes(b"%2f") {
        return Some(Unmatchable::EncodedSlash);
    }
    if path.split('/').any(|segment| matches!(segment, "." | "..")) {
        return Some(Unmatchable::DotSegment);
    }
    if path.contains("//") {
        return Some(Unmatchable::EmptySegment);
    }
    None
}

pub fn reported_method(line: &RequestLine) -> ReportedMethod {
    STANDARD_METHODS
        .iter()
        .find(|standard| **standard == line.method())
        .map_or(ReportedMethod::Other, |standard| {
            ReportedMethod::Standard(standard)
        })
}

/// The redacted path's first `segments` segments, `/` for none.
pub fn path_prefix(line: &RequestLine, segments: usize) -> &str {
    // The redacted path starts with `/` and has no empty segment.
    let path = line.redacted_path();
    let end = path
        .match_indices('/')
        .nth(segments)
        .map_or(path.len(), |(at, _)| at);
    &path[..end.max(1)]
}
