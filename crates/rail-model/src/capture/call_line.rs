//! A call's request line as capture reads it: what its conditions compare, and what a call record
//! reports of it.

use super::redaction::redact_path;
use crate::authority::split_authority;
use crate::condition::RequestLine;

// What a host that can't be reported reads as, compared and reported.
const UNPARSEABLE: &str = "[unparseable]";

// What a method outside the standard nine is reported as.
const OTHER_METHOD: &str = "[other]";

// DNS's longest name.
const MAX_HOST_CHARS: usize = 253;

const STANDARD_METHODS: [&str; 9] = [
    "GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH",
];

/// A call's request line, read once.
#[derive(Clone, Debug)]
pub struct CallLine {
    request_line: RequestLine,
    host_reading: HostReading,
    unmatchable: Option<Unmatchable>,
}

/// How a call's host was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostReading {
    /// Spelled as `split_authority` spells it.
    Read,
    /// An authority `split_authority` refuses, or one that isn't UTF-8: `[unparseable]`.
    Refused,
    /// Past DNS's longest name: `[unparseable]`, so its length can't carry data.
    PastLongest,
}

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
    /// An empty segment other than a trailing one.
    EmptySegment,
}

/// The method a call record reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportedMethod<'a> {
    /// One of the nine standard methods, as sent.
    Standard(&'a str),
    /// Any other, so a custom method can't carry data: `[other]`.
    Other,
}

impl ReportedMethod<'_> {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Standard(method) => method,
            Self::Other => OTHER_METHOD,
        }
    }
}

impl CallLine {
    /// From a call's `:authority`, `:method` and `:path` as sent: the path up to its first `?` or
    /// `#`, and `/` for a `CONNECT`.
    pub fn read(authority: &[u8], method: &[u8], path: &[u8]) -> Self {
        let (host, host_reading) = read_host(authority);
        let method = String::from_utf8_lossy(method).into_owned();
        let path = if method == "CONNECT" { b"/" } else { path };
        let path = path
            .split(|byte| matches!(byte, b'?' | b'#'))
            .next()
            .unwrap_or_default();
        let (path, unmatchable) = match std::str::from_utf8(path) {
            Ok(path) => (path.to_owned(), unmatchable(path)),
            Err(_) => (
                String::from_utf8_lossy(path).into_owned(),
                Some(Unmatchable::NotUtf8),
            ),
        };
        Self {
            request_line: RequestLine {
                host,
                method,
                redacted_path: redact_path(&path),
                path,
            },
            host_reading,
            unmatchable,
        }
    }

    /// What a condition reads under `request`.
    pub fn request_line(&self) -> &RequestLine {
        &self.request_line
    }

    pub fn host_reading(&self) -> HostReading {
        self.host_reading
    }

    /// Why the path matches no entry, if it can't.
    pub fn unmatchable(&self) -> Option<Unmatchable> {
        self.unmatchable
    }

    pub fn reported_method(&self) -> ReportedMethod<'_> {
        let method = self.request_line.method.as_str();
        if STANDARD_METHODS.contains(&method) {
            ReportedMethod::Standard(method)
        } else {
            ReportedMethod::Other
        }
    }

    /// The redacted path's first `segments` segments, `/` for none.
    pub fn path_prefix(&self, segments: usize) -> &str {
        // The redacted path starts with `/` and has no empty segment.
        let path = &self.request_line.redacted_path;
        let end = path
            .match_indices('/')
            .nth(segments)
            .map_or(path.len(), |(at, _)| at);
        &path[..end.max(1)]
    }
}

fn read_host(authority: &[u8]) -> (String, HostReading) {
    let read = std::str::from_utf8(authority)
        .ok()
        .and_then(|authority| split_authority(authority).ok());
    match read {
        None => (UNPARSEABLE.to_owned(), HostReading::Refused),
        // ASCII, so its length in bytes is in characters.
        Some((host, _)) if host.len() > MAX_HOST_CHARS => {
            (UNPARSEABLE.to_owned(), HostReading::PastLongest)
        }
        Some((host, _)) => (host, HostReading::Read),
    }
}

// On the path as received, before redaction.
fn unmatchable(path: &str) -> Option<Unmatchable> {
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
    // Two slashes in a row are an empty segment before another.
    if path.contains("//") {
        return Some(Unmatchable::EmptySegment);
    }
    None
}
