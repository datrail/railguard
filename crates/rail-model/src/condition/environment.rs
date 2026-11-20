use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::body::BodyRead;
use crate::endpoint::Endpoint;

// Capture fills neither: a plan's check refuses the paths that read them.
static NO_ENDPOINT: Endpoint = Endpoint::Keyless;
static NO_HEADERS: Headers = Headers(BTreeMap::new());

/// What a condition reads about one call.
#[derive(Clone, Copy, Debug)]
pub struct Environment<'a> {
    pub(crate) ticket: Option<&'a Map<String, Value>>,
    pub(crate) endpoint: &'a Endpoint,
    pub(crate) headers: &'a Headers,
    pub(crate) request: Option<&'a CaptureRequest>,
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
            ticket,
            endpoint,
            headers,
            request: None,
            body: None,
        }
    }

    /// For capture to match a call against a plan: its request line and its body.
    pub fn for_capture(request: &'a CaptureRequest, body: &'a BodyRead) -> Self {
        Self {
            ticket: None,
            endpoint: &NO_ENDPOINT,
            headers: &NO_HEADERS,
            request: Some(request),
            body: Some(body),
        }
    }
}

/// What capture reads of a call's request line, as the caller spells each.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptureRequest {
    pub host: String,
    pub method: String,
    /// Up to its first `?` or `#`, as received.
    pub path: String,
    pub redacted_path: String,
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
