//! An agent's request as a host received it, and the rules its headers and path are read by.

use std::fmt;
use std::sync::OnceLock;

use percent_encoding::percent_decode_str;

use crate::body::mcp::read_endpoint;
use crate::body::{BodyRead, Unread};
use crate::condition::Headers;
use crate::endpoint::Resolution;
use crate::request_line::RequestLine;

/// The ticket, for the gateway to check.
pub const XRAIL: &str = "x-rail";

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
    /// Every byte of the body the host was sent, kept or not.
    pub(crate) body_bytes: u64,
    // Each read once, by whichever use asks first, on whichever thread.
    request_line: OnceLock<RequestLine>,
    headers: OnceLock<Headers>,
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

    /// Its header lines as conditions read them.
    pub(crate) fn headers(&self) -> &Headers {
        self.headers.get_or_init(|| {
            Headers::from_lines(
                self.header_lines
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str())),
            )
        })
    }

    /// What its decoded path and MCP message name, unless its body is unread.
    pub(crate) fn endpoint(&self) -> Result<&Resolution, Unread> {
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

/// Whether an agent's header is kept from the upstream: an x-rail header, or any name with `_`.
/// Takes a lowercased name.
pub fn is_withheld_header(name: &str) -> bool {
    // Some upstreams read `_` and `-` alike, so `x_rail` could pass for `x-rail`.
    name == XRAIL || name.starts_with("x-rail-") || name.contains('_')
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
