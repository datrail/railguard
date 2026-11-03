use std::collections::BTreeMap;
use std::str::FromStr;

use crate::ticket::TicketClaims;

/// What a condition reads about one call.
#[derive(Clone, Copy, Debug)]
pub struct Environment<'a> {
    /// `None` without a usable ticket: every `ticket` path then reads as absent.
    pub ticket: Option<&'a TicketClaims>,
    pub endpoint: &'a Endpoint,
    pub headers: &'a Headers,
}

/// Rail Center caps a key at 255 characters, so a longer one is never bound.
const MAX_KEY_CHARS: usize = 255;

/// Messages that open a session or describe it, besides `notifications/…`.
const DISCOVERY_METHODS: [&str; 6] = [
    "initialize",
    "ping",
    "tools/list",
    "prompts/list",
    "resources/list",
    "resources/templates/list",
];

/// Whether an MCP message is judged, and what it names.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// Forwarded unjudged: judging it would let a rule bound to one tool close the session.
    Discovery,
    Judged(Endpoint),
}

/// What a judged message names.
#[derive(Clone, Debug, PartialEq)]
pub enum Endpoint {
    /// A method naming no tool, such as `resources/read`. No binding fallback applies.
    Keyless,
    /// A `tools/call` naming no tool this reader can key. The binding fallback applies.
    Unrecognised,
    Resolved(ResolvedKey),
}

/// From a message's method and tool name, and the path the upstream serves it at.
pub fn resolve_endpoint(method: &str, tool_name: Option<&str>, path: &str) -> Resolution {
    if DISCOVERY_METHODS.contains(&method) || method.starts_with("notifications/") {
        return Resolution::Discovery;
    }
    if method != "tools/call" {
        return Resolution::Judged(Endpoint::Keyless);
    }
    let Some(tool_name) = tool_name.filter(|name| !name.is_empty()) else {
        return Resolution::Judged(Endpoint::Unrecognised);
    };
    let text = format!("{path}#{method}#{tool_name}");
    if text.chars().count() > MAX_KEY_CHARS {
        return Resolution::Judged(Endpoint::Unrecognised);
    }
    match text.parse() {
        Ok(key) => Resolution::Judged(Endpoint::Resolved(key)),
        Err(_) => Resolution::Judged(Endpoint::Unrecognised),
    }
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
