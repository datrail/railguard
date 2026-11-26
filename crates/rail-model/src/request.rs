//! An agent's request as a host received it, and the rules its headers and path are read by.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

use percent_encoding::percent_decode_str;
use serde_json::Value;

use crate::body::mcp::read_endpoint;
use crate::body::{BodyRead, Charge, Unread};
use crate::endpoint::Resolution;
use crate::request_line::RequestLine;

/// An agent's request as a host received it, before any rewrite.
pub struct ObservedRequest {
    /// `:authority` as the agent sent it, before the proxy rewrites it.
    pub(crate) authority: WireText,
    pub(crate) method: WireText,
    /// `:path` as received: the path and its query, never decoded.
    pub(crate) path: WireText,
    /// The header lines the host passes, in order.
    pub(crate) header_lines: Vec<(WireText, WireText)>,
    pub(crate) body: BodyRead,
    /// The body's length: as declared, or the bytes received if more.
    pub(crate) body_bytes: u64,
    // Each read once, by whichever use asks first, on whichever thread.
    request_line: OnceLock<RequestLine>,
    // `None` past the budget; else the headers and the charge they hold.
    headers: OnceLock<Option<(Headers, Option<Charge>)>>,
    endpoint: OnceLock<Resolution>,
}

impl ObservedRequest {
    /// From each part as sent, `headers` its lines in order.
    pub fn new(
        authority: Vec<u8>,
        method: Vec<u8>,
        path: Vec<u8>,
        headers: Vec<(Vec<u8>, Vec<u8>)>,
        body: BodyRead,
        body_bytes: u64,
    ) -> Self {
        Self {
            authority: authority.into(),
            method: method.into(),
            path: path.into(),
            header_lines: headers
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
            body,
            body_bytes,
            request_line: OnceLock::new(),
            headers: OnceLock::new(),
            endpoint: OnceLock::new(),
        }
    }

    /// Its method as text, bytes that aren't UTF-8 replaced.
    pub fn method(&self) -> &str {
        self.method.as_str()
    }

    /// Its header lines as text, in order, bytes that aren't UTF-8 replaced.
    pub fn header_lines(&self) -> impl Iterator<Item = (&str, &str)> {
        self.header_lines
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    pub fn body(&self) -> &BodyRead {
        &self.body
    }

    /// The body's length: as declared, or the bytes received if more.
    pub fn body_bytes(&self) -> u64 {
        self.body_bytes
    }

    /// Its authority, method and path as matching reads them.
    pub(crate) fn request_line(&self) -> &RequestLine {
        self.request_line.get_or_init(|| {
            RequestLine::read(
                self.authority.as_bytes(),
                self.method.as_bytes(),
                self.path.as_bytes(),
            )
        })
    }

    /// Its header lines as conditions read them, its authority as `host` where no `host` line was
    /// sent: rules read `host` whether the request came as HTTP/1 or HTTP/2. Charged to its body's
    /// budget if its body is whole; `None` past it, or for a body kept outside it.
    pub(crate) fn headers(&self) -> Option<&Headers> {
        let held = self.headers.get_or_init(|| {
            let sent = self
                .header_lines()
                .any(|(name, _)| name.eq_ignore_ascii_case("host"));
            let authority = (!self.authority.as_bytes().is_empty() && !sent)
                .then(|| ("host", self.authority.as_str()));
            let lines = || self.header_lines().chain(authority);
            let charge = match &self.body {
                BodyRead::Whole(body) => Some(body.charge_for(Headers::bytes_for(lines()))?),
                BodyRead::Unread(_) => None,
            };
            Some((Headers::from_lines(lines()), charge))
        });
        held.as_ref().map(|(headers, _)| headers)
    }

    /// What its decoded path and MCP message name, unless its body is unread.
    pub fn endpoint(&self) -> Result<&Resolution, Unread> {
        match &self.body {
            BodyRead::Unread(why) => Err(*why),
            BodyRead::Whole(body) => Ok(self
                .endpoint
                .get_or_init(|| read_endpoint(&read_path(self.path.as_str()), body.mcp()))),
        }
    }
}

// Neither its path nor its headers, which can carry credentials.
impl fmt::Debug for ObservedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservedRequest")
            .field("authority", &self.authority.as_str())
            .field("method", &self.method.as_str())
            .field("body", &self.body)
            .field("body_bytes", &self.body_bytes)
            .finish_non_exhaustive()
    }
}

/// Text as read from the wire: its bytes as sent, and as text, those that aren't UTF-8 replaced.
pub(crate) enum WireText {
    Text(String),
    NotText { raw: Vec<u8>, lossy: String },
}

impl From<Vec<u8>> for WireText {
    // Without a copy, unless it isn't UTF-8.
    fn from(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(text) => Self::Text(text),
            Err(error) => {
                let raw = error.into_bytes();
                let lossy = String::from_utf8_lossy(&raw).into_owned();
                Self::NotText { raw, lossy }
            }
        }
    }
}

impl WireText {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::NotText { raw, .. } => raw,
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Text(text) | Self::NotText { lossy: text, .. } => text,
        }
    }
}

/// The path of `uri`, without the query, percent-decoded: an invalid escape is kept, and invalid
/// UTF-8 replaced.
pub fn read_path(uri: &str) -> String {
    let path = uri.split_once('?').map_or(uri, |(path, _)| path);
    let path = percent_decode_str(path).decode_utf8_lossy();
    if path.is_empty() {
        "/".to_owned()
    } else {
        path.into_owned()
    }
}

/// A call's headers as conditions read them: names lowercased, every value split on commas.
/// Held as JSON strings, so a header's values read like a list in the ticket.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Headers(BTreeMap<String, Vec<Value>>);

impl Headers {
    /// From each line as sent, in order. Lines sharing a name pool their parts.
    pub fn from_lines<N, V>(lines: impl IntoIterator<Item = (N, V)>) -> Self
    where
        N: AsRef<str>,
        V: AsRef<str>,
    {
        let mut headers: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for (name, value) in lines {
            let parts = headers.entry(name.as_ref().to_lowercase()).or_default();
            // Differs from Rail Center's `strip` only on control characters a header value can't hold.
            parts.extend(
                value
                    .as_ref()
                    .split(',')
                    .map(|part| Value::String(part.trim().to_string())),
            );
        }
        Self(headers)
    }

    // About what `from_lines` takes for `lines`, charged before it's built.
    fn bytes_for<'l>(lines: impl Iterator<Item = (&'l str, &'l str)>) -> usize {
        let part_bytes = size_of::<Value>() + size_of::<String>();
        lines
            .map(|(name, value)| {
                let parts = value.bytes().filter(|&byte| byte == b',').count() + 1;
                name.len() + size_of::<(String, Vec<Value>)>() + value.len() + parts * part_bytes
            })
            .sum()
    }

    pub(crate) fn get(&self, name: &str) -> Option<&[Value]> {
        self.0.get(name).map(Vec::as_slice)
    }
}
