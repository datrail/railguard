use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::endpoint::Endpoint;

/// What a condition reads about one call.
#[derive(Clone, Copy, Debug)]
pub struct Environment<'a> {
    pub(crate) ticket: Option<&'a Map<String, Value>>,
    pub(crate) endpoint: &'a Endpoint,
    pub(crate) headers: &'a Headers,
}

impl<'a> Environment<'a> {
    /// A call's ticket claims (`None` without a usable ticket), its endpoint and its headers.
    pub fn new(
        ticket: Option<&'a Map<String, Value>>,
        endpoint: &'a Endpoint,
        headers: &'a Headers,
    ) -> Self {
        Self {
            ticket,
            endpoint,
            headers,
        }
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
