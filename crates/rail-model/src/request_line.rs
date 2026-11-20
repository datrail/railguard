//! A call's request line, read once, its host and path spelled one way wherever they're compared,
//! and what a call record reports of it.

use std::borrow::Cow;
use std::fmt;

use crate::authority::split_authority;
use crate::redaction::redact_path;

// What a host that can't be read reads as, compared and reported.
const UNPARSEABLE: &str = "[unparseable]";

// DNS's longest name.
const MAX_HOST_CHARS: usize = 253;

// What a method outside the standard nine is reported as.
const OTHER_METHOD: &str = "[other]";

const STANDARD_METHODS: [&str; 9] = [
    "GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH",
];

/// A call's host, method and path, as conditions read them under `request`.
pub struct RequestLine {
    host: HostReading,
    method: String,
    path: String,
    path_lossy: bool,
    redacted_path: String,
}

/// How a call's host was read.
#[derive(Debug, PartialEq, Eq)]
pub enum HostReading {
    /// Spelled as `split_authority` spells it.
    Read(String),
    /// An authority `split_authority` refuses, or one that isn't UTF-8: `[unparseable]`.
    Refused,
    /// Past DNS's longest name: `[unparseable]`, so its length can't carry data.
    PastLongest,
}

/// Why a request path matches no capture plan entry, whatever its conditions say.
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
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Standard(method) => method,
            Self::Other => OTHER_METHOD,
        }
    }
}

impl RequestLine {
    /// From a call's `:authority`, `:method` and `:path` as sent: the path up to its first `?` or
    /// `#`, and `/` for a `CONNECT`.
    pub fn read(authority: &[u8], method: &[u8], path: &[u8]) -> Self {
        let host = read_host(authority);
        let method = String::from_utf8_lossy(method).into_owned();
        let path = if method == "CONNECT" { b"/" } else { path };
        let path = path
            .split(|byte| matches!(byte, b'?' | b'#'))
            .next()
            .unwrap_or_default();
        let path = String::from_utf8_lossy(path);
        // Borrowed exactly when the bytes are UTF-8.
        let path_lossy = matches!(path, Cow::Owned(_));
        let path = path.into_owned();
        Self {
            host,
            method,
            redacted_path: redact_path(&path),
            path,
            path_lossy,
        }
    }

    /// As `split_authority` spells it, or `[unparseable]`.
    pub fn host(&self) -> &str {
        match &self.host {
            HostReading::Read(host) => host,
            HostReading::Refused | HostReading::PastLongest => UNPARSEABLE,
        }
    }

    /// Whether `host` is the name, or `[unparseable]` and why.
    pub fn host_reading(&self) -> &HostReading {
        &self.host
    }

    /// As sent.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// As received, never percent-decoded; bytes that aren't UTF-8 replaced.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The path by RailMon's redaction rule, as Rail Center writes a plan's paths.
    pub fn redacted_path(&self) -> &str {
        &self.redacted_path
    }

    /// Why the path matches no plan entry, if it can't. On the path as received, before redaction.
    pub fn unmatchable(&self) -> Option<Unmatchable> {
        if self.path_lossy {
            return Some(Unmatchable::NotUtf8);
        }
        let path = self.path.as_str();
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

    /// The method a call record reports.
    pub fn reported_method(&self) -> ReportedMethod {
        STANDARD_METHODS
            .into_iter()
            .find(|standard| *standard == self.method)
            .map_or(ReportedMethod::Other, ReportedMethod::Standard)
    }

    /// The redacted path's first `segments` segments, `/` for none.
    pub fn path_prefix(&self, segments: usize) -> &str {
        // The redacted path starts with `/` and has no empty segment.
        let path = &self.redacted_path;
        let end = path
            .match_indices('/')
            .nth(segments)
            .map_or(path.len(), |(at, _)| at);
        &path[..end.max(1)]
    }
}

// The path as received can hold a secret.
impl fmt::Debug for RequestLine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestLine")
            .field("host", &self.host)
            .field("method", &self.method)
            .field("redacted_path", &self.redacted_path)
            .finish_non_exhaustive()
    }
}

fn read_host(authority: &[u8]) -> HostReading {
    let read = std::str::from_utf8(authority)
        .ok()
        .and_then(|authority| split_authority(authority).ok());
    match read {
        None => HostReading::Refused,
        // ASCII, so its length in bytes is in characters.
        Some((host, _)) if host.len() > MAX_HOST_CHARS => HostReading::PastLongest,
        Some((host, _)) => HostReading::Read(host),
    }
}
