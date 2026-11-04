use std::collections::BTreeMap;
use std::str::FromStr;

use crate::ticket::TicketClaims;

/// What a condition reads about one call.
#[derive(Clone, Copy, Debug)]
pub struct Environment<'a> {
    /// `None` without a usable ticket: every `ticket` path then reads as absent.
    pub ticket: Option<&'a TicketClaims>,
    /// `None` for a call naming no endpoint.
    pub endpoint_key: Option<&'a ResolvedKey>,
    pub headers: &'a Headers,
}

/// An endpoint key as a host resolves it from a call: `<path>#<method>#<call>`, without the slug.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResolvedKey {
    text: String,
    path_len: usize,
    method_len: usize,
}

impl ResolvedKey {
    pub fn path(&self) -> &str {
        &self.text[..self.path_len]
    }

    pub fn method(&self) -> &str {
        &self.text[self.path_len + 1..self.path_len + 1 + self.method_len]
    }

    pub fn call(&self) -> &str {
        &self.text[self.path_len + self.method_len + 2..]
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl FromStr for ResolvedKey {
    type Err = String;

    /// Only `call` can contain a `#`, and no part is empty.
    fn from_str(text: &str) -> Result<Self, String> {
        let parts: Vec<&str> = text.splitn(3, '#').collect();
        let [path, method, call] = parts[..] else {
            return Err(format!("endpoint key {text:?}"));
        };
        if path.is_empty() || method.is_empty() || call.is_empty() {
            return Err(format!("endpoint key {text:?}"));
        }
        Ok(Self {
            text: text.to_string(),
            path_len: path.len(),
            method_len: method.len(),
        })
    }
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
