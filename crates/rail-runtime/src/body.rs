//! A call's body, read in stages: as sent, decoded, as JSON, and what it names.

use std::borrow::Cow;
use std::fmt;

use rail_policy::{Endpoint, Resolution, resolve_endpoint};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

// Past this, a body names nothing: as the Python gateway, so a call judged there is judged here.
const MAX_NESTING: usize = 256;

/// The bytes a reader reads. As sent for now: `Content-Encoding` isn't decoded, as in Python.
pub(crate) fn decode_body(sent: &[u8]) -> Cow<'_, [u8]> {
    Cow::Borrowed(sent)
}

/// A body read as one JSON object: its keys and the last value of each, values only scanned, so
/// no number in them is read. Read as the Python gateway's `json.loads` reads it, where it matters.
pub(crate) struct JsonBody<'a> {
    // In order; a key repeated keeps its last value.
    members: Vec<(Cow<'a, [u8]>, &'a RawValue)>,
}

impl<'a> JsonBody<'a> {
    /// `None` for a body that isn't one JSON object in UTF-8, or nests past 256 levels.
    pub(crate) fn parse(body: &'a [u8]) -> Option<Self> {
        // Python's `json.loads` reads bytes past one UTF-8 BOM.
        let body = body.strip_prefix("\u{feff}".as_bytes()).unwrap_or(body);
        if nests_past(body, MAX_NESTING) {
            return None;
        }
        Self::object(body)
    }

    /// The value of `key`, its last if repeated.
    pub(crate) fn get(&self, key: &str) -> Option<&'a RawValue> {
        let key = key.as_bytes();
        let (_, value) = self.members.iter().rev().find(|(name, _)| &**name == key)?;
        Some(value)
    }

    fn object(json: &'a [u8]) -> Option<Self> {
        let mut deserializer = serde_json::Deserializer::from_slice(json);
        let members = deserializer.deserialize_map(Members).ok()?;
        deserializer.end().ok()?;
        Some(Self { members })
    }
}

/// What a body sent to `path` asks for. One that isn't a JSON object names no endpoint this reader
/// can key; a `method` or `name` of another type reads as absent, as in Python.
pub(crate) fn read_endpoint(path: &str, body: Option<&JsonBody>) -> Resolution {
    let Some(body) = body else {
        return Resolution::Judged(Endpoint::Unrecognised);
    };
    let method = body.get("method").and_then(text);
    let name = body
        .get("params")
        .and_then(|params| JsonBody::object(params.get().as_bytes()))
        .and_then(|params| params.get("name"))
        .and_then(text);
    resolve_endpoint(path, method.as_deref().unwrap_or(""), name.as_deref())
}

// A string's text; `None` for any other value.
fn text(value: &RawValue) -> Option<String> {
    serde_json::from_str(value.get()).ok()
}

// Brackets outside strings: counted before parsing, as the Python gateway does.
fn nests_past(body: &[u8], limit: usize) -> bool {
    let mut depth = 0_usize;
    let (mut in_string, mut escaped) = (false, false);
    for &byte in body {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

// Visits an object, keeping each key and its value as sent.
struct Members;

impl<'de> Visitor<'de> for Members {
    type Value = Vec<(Cow<'de, [u8]>, &'de RawValue)>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut members = Vec::new();
        while let Some(Key(key)) = map.next_key()? {
            members.push((key, map.next_value()?));
        }
        Ok(members)
    }
}

// A key as bytes, so one holding a lone surrogate escape fails nothing, as in Python.
struct Key<'a>(Cow<'a, [u8]>);

impl<'de> serde::Deserialize<'de> for Key<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_bytes(KeyBytes)
    }
}

struct KeyBytes;

impl<'de> Visitor<'de> for KeyBytes {
    type Value = Key<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a key")
    }

    fn visit_borrowed_bytes<E: serde::de::Error>(self, key: &'de [u8]) -> Result<Self::Value, E> {
        Ok(Key(Cow::Borrowed(key)))
    }

    fn visit_bytes<E: serde::de::Error>(self, key: &[u8]) -> Result<Self::Value, E> {
        Ok(Key(Cow::Owned(key.to_vec())))
    }

    fn visit_borrowed_str<E: serde::de::Error>(self, key: &'de str) -> Result<Self::Value, E> {
        Ok(Key(Cow::Borrowed(key.as_bytes())))
    }

    fn visit_str<E: serde::de::Error>(self, key: &str) -> Result<Self::Value, E> {
        Ok(Key(Cow::Owned(key.as_bytes().to_vec())))
    }
}
