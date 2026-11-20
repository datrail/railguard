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
    host: HostReading,
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
    /// From a call's `:authority`, `:method` and `:path` as sent. A `CONNECT`'s path is `/`.
    pub fn read(authority: &[u8], method: &[u8], path: &[u8]) -> Self {
        let (host, host_reading) = read_host(authority);
        let method = String::from_utf8_lossy(method).into_owned();
        let path = if method == "CONNECT" { b"/" } else { path };
        let path = &path[..path
            .iter()
            .position(|byte| matches!(byte, b'?' | b'#'))
            .unwrap_or(path.len())];
        let unmatchable = unmatchable(path);
        let path = String::from_utf8_lossy(path).into_owned();
        Self {
            request_line: RequestLine {
                host,
                method,
                redacted_path: redact_path(&path),
                path,
            },
            host: host_reading,
            unmatchable,
        }
    }

    /// What a condition reads under `request`.
    pub fn request_line(&self) -> &RequestLine {
        &self.request_line
    }

    pub fn host(&self) -> HostReading {
        self.host
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
    pub fn path_prefix(&self, segments: usize) -> String {
        let kept: Vec<&str> = self
            .request_line
            .redacted_path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .take(segments)
            .collect();
        format!("/{}", kept.join("/"))
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
fn unmatchable(path: &[u8]) -> Option<Unmatchable> {
    let Ok(path) = std::str::from_utf8(path) else {
        return Some(Unmatchable::NotUtf8);
    };
    let lower = path.to_ascii_lowercase();
    if path.contains(['[', ']']) || lower.contains("%5b") || lower.contains("%5d") {
        return Some(Unmatchable::Bracket);
    }
    if lower.contains("%2f") {
        return Some(Unmatchable::EncodedSlash);
    }
    let segments: Vec<&str> = path.strip_prefix('/').unwrap_or(path).split('/').collect();
    if segments
        .iter()
        .any(|segment| matches!(*segment, "." | ".."))
    {
        return Some(Unmatchable::DotSegment);
    }
    if segments[..segments.len() - 1]
        .iter()
        .any(|segment| segment.is_empty())
    {
        return Some(Unmatchable::EmptySegment);
    }
    None
}
