//! A call's request line, read once, its host and path spelled one way wherever they're compared.

use std::fmt;

use crate::authority::split_authority;
use crate::redaction::redact_path;

// What a host that can't be read reads as.
const UNPARSEABLE: &str = "[unparseable]";

// DNS's longest name.
const MAX_HOST_CHARS: usize = 253;

/// A call's host, method and path, as conditions read them under `request`.
#[derive(Clone, PartialEq)]
pub struct RequestLine {
    host: String,
    host_reading: HostReading,
    method: String,
    path: String,
    path_lossy: bool,
    redacted_path: String,
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

impl RequestLine {
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
        let (path, path_lossy) = match std::str::from_utf8(path) {
            Ok(path) => (path.to_owned(), false),
            Err(_) => (String::from_utf8_lossy(path).into_owned(), true),
        };
        Self {
            host,
            host_reading,
            method,
            redacted_path: redact_path(&path),
            path,
            path_lossy,
        }
    }

    /// As `split_authority` spells it, or `[unparseable]`.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn host_reading(&self) -> HostReading {
        self.host_reading
    }

    /// As sent.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// As received, never decoded.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Whether the path wasn't UTF-8, so `path` replaced what wasn't.
    pub fn path_lossy(&self) -> bool {
        self.path_lossy
    }

    /// The path by RailMon's redaction rule, as Rail Center writes a plan's paths.
    pub fn redacted_path(&self) -> &str {
        &self.redacted_path
    }
}

// The path as received can hold a secret.
impl fmt::Debug for RequestLine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestLine")
            .field("host", &self.host)
            .field("host_reading", &self.host_reading)
            .field("method", &self.method)
            .field("redacted_path", &self.redacted_path)
            .finish_non_exhaustive()
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
