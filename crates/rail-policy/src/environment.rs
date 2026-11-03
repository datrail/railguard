use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::endpoint::Endpoint;

/// What a condition reads about one call.
#[derive(Clone, Copy, Debug)]
pub struct Environment<'a> {
    /// A usable ticket's claims. `None` without one: every `ticket` path then reads as absent.
    pub ticket: Option<&'a Map<String, Value>>,
    pub endpoint: &'a Endpoint,
    pub headers: &'a Headers,
}

/// A call's headers as conditions read them: names lowercased, every value split on commas.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Headers(BTreeMap<String, Vec<String>>);

impl Headers {
    /// From each line as sent, in order. Lines sharing a name pool their parts.
    pub fn from_lines<N, V>(lines: impl IntoIterator<Item = (N, V)>) -> Self
    where
        N: AsRef<str>,
        V: AsRef<str>,
    {
        let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, value) in lines {
            let parts = headers.entry(name.as_ref().to_lowercase()).or_default();
            // Differs from Rail Center's `strip` only on control characters a header value can't hold.
            parts.extend(
                value
                    .as_ref()
                    .split(',')
                    .map(|part| part.trim().to_string()),
            );
        }
        Self(headers)
    }

    pub(crate) fn get(&self, name: &str) -> Option<&[String]> {
        self.0.get(name).map(Vec::as_slice)
    }
}
