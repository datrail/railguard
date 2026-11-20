use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value};

use crate::body::BodyRead;
use crate::endpoint::Endpoint;

/// What a condition reads about one call. Each use fills its own sources; a condition compiled
/// for that use reads no other.
#[derive(Clone, Copy, Debug)]
pub struct Environment<'a> {
    /// Filled with `None` where the call has no usable ticket.
    pub(crate) ticket: Option<Option<&'a Map<String, Value>>>,
    pub(crate) endpoint: Option<&'a Endpoint>,
    pub(crate) headers: Option<&'a Headers>,
    pub(crate) request_line: Option<&'a RequestLine>,
    pub(crate) body: Option<&'a BodyRead>,
}

impl<'a> Environment<'a> {
    /// For judging a call against the policy bundle: its ticket claims (`None` without a usable
    /// ticket), its endpoint and its headers.
    pub fn for_judging(
        ticket: Option<&'a Map<String, Value>>,
        endpoint: &'a Endpoint,
        headers: &'a Headers,
    ) -> Self {
        Self {
            ticket: Some(ticket),
            endpoint: Some(endpoint),
            headers: Some(headers),
            request_line: None,
            body: None,
        }
    }

    /// For capture to match a call against a plan: its request line and its body.
    pub fn for_capture(request_line: &'a RequestLine, body: &'a BodyRead) -> Self {
        Self {
            ticket: None,
            endpoint: None,
            headers: None,
            request_line: Some(request_line),
            body: Some(body),
        }
    }
}

/// A call's request line, as the caller spells each part.
#[derive(Clone, Default, PartialEq)]
pub struct RequestLine {
    pub host: String,
    pub method: String,
    /// Up to its first `?` or `#`, as received.
    pub path: String,
    pub redacted_path: String,
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

    pub(crate) fn get(&self, name: &str) -> Option<&[Value]> {
        self.0.get(name).map(Vec::as_slice)
    }
}
