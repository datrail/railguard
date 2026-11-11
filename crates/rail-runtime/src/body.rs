//! What an MCP request body names: its method and tool.
//!
//! Not parsed to a `Value`, which refuses a number past a double or a lone surrogate, nor to a derived
//! struct, which refuses a repeated key or a key holding a lone surrogate: other JSON readers take
//! them all, so refusing one would let a caller hide the call's endpoint. Values are only scanned, and only the two
//! strings read are parsed.

use std::borrow::Cow;
use std::fmt;

use rail_policy::{Endpoint, Resolution, resolve_endpoint};
use serde::de::{DeserializeSeed, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

// Past this, a body names nothing.
const MAX_NESTING: usize = 256;

/// What `body`, as sent to `path`, asks for. One that isn't a JSON object in UTF-8, or nests past
/// 256 levels, names no endpoint this reader can key; a `method` or `name` of another type reads as
/// absent. `Content-Encoding` isn't decoded.
pub(crate) fn read_endpoint(path: &str, body: &[u8]) -> Resolution {
    // Read past one UTF-8 BOM, which a JSON reader may accept.
    let body = body.strip_prefix("\u{feff}".as_bytes()).unwrap_or(body);
    if nests_past(body, MAX_NESTING) {
        return Resolution::Judged(Endpoint::Unrecognised);
    }
    let Some(message) = members(body) else {
        return Resolution::Judged(Endpoint::Unrecognised);
    };
    let method = last(&message, "method").and_then(text);
    let name = last(&message, "params")
        .and_then(|params| members(params.get().as_bytes()))
        .and_then(|params| last(&params, "name"))
        .and_then(text);
    resolve_endpoint(path, method.as_deref().unwrap_or(""), name.as_deref())
}

// An object's keys and values, in order, each value only scanned.
type Members<'a> = Vec<(Cow<'a, [u8]>, &'a RawValue)>;

fn members(json: &[u8]) -> Option<Members<'_>> {
    let mut deserializer = serde_json::Deserializer::from_slice(json);
    let members = deserializer.deserialize_map(Object).ok()?;
    deserializer.end().ok()?;
    Some(members)
}

// A key repeated keeps its last value.
fn last<'a>(members: &Members<'a>, key: &str) -> Option<&'a RawValue> {
    let key = key.as_bytes();
    let (_, value) = members.iter().rev().find(|(name, _)| &**name == key)?;
    Some(value)
}

// A string's text; `None` for any other value.
fn text(value: &RawValue) -> Option<String> {
    serde_json::from_str(value.get()).ok()
}

// Brackets outside strings, counted before parsing, so a deep body can't exhaust the parser.
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
struct Object;

impl<'de> Visitor<'de> for Object {
    type Value = Members<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut members = Vec::new();
        while let Some(key) = map.next_key_seed(KeyBytes)? {
            members.push((key, map.next_value()?));
        }
        Ok(members)
    }
}

// A key as bytes, so one holding a lone surrogate escape fails nothing.
struct KeyBytes;

impl<'de> DeserializeSeed<'de> for KeyBytes {
    type Value = Cow<'de, [u8]>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_bytes(self)
    }
}

impl<'de> Visitor<'de> for KeyBytes {
    type Value = Cow<'de, [u8]>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a key")
    }

    fn visit_borrowed_bytes<E: serde::de::Error>(self, key: &'de [u8]) -> Result<Self::Value, E> {
        Ok(Cow::Borrowed(key))
    }

    fn visit_bytes<E: serde::de::Error>(self, key: &[u8]) -> Result<Self::Value, E> {
        Ok(Cow::Owned(key.to_vec()))
    }

    // Not called by serde_json today; kept so a key handed over as text still reads.
    fn visit_borrowed_str<E: serde::de::Error>(self, key: &'de str) -> Result<Self::Value, E> {
        Ok(Cow::Borrowed(key.as_bytes()))
    }

    fn visit_str<E: serde::de::Error>(self, key: &str) -> Result<Self::Value, E> {
        Ok(Cow::Owned(key.as_bytes().to_vec()))
    }
}
