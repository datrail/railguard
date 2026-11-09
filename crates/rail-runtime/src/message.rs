//! What an MCP request body names: its method and tool, read as the Python gateway reads them.

use std::borrow::Cow;
use std::fmt;

use rail_policy::{Endpoint, Resolution, resolve_endpoint};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

// Past this, a body names nothing: as the Python gateway, so a call judged there is judged here.
const MAX_NESTING: usize = 256;

/// What `body`, sent to `path`, asks for. A body that isn't a JSON object, or nests past 256
/// levels, names no endpoint this reader can key.
pub(crate) fn resolve_body(path: &str, body: &str) -> Resolution {
    // Python's `json.loads` reads bytes past one UTF-8 BOM.
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    if nests_past(body, MAX_NESTING) {
        return Resolution::Judged(Endpoint::Unrecognised);
    }
    let Some([method, params]) = last_values(body, ["method", "params"]) else {
        return Resolution::Judged(Endpoint::Unrecognised);
    };
    // A value of another type reads as absent, and a key repeated keeps its last, as in Python.
    let method = method.and_then(text);
    let name = params
        .and_then(|params| last_values(params.get(), ["name"]))
        .and_then(|[name]| name)
        .and_then(text);
    resolve_endpoint(path, method.as_deref().unwrap_or(""), name.as_deref())
}

// Brackets outside strings: counted before parsing, as the Python gateway does.
fn nests_past(body: &str, limit: usize) -> bool {
    let mut depth = 0_usize;
    let (mut in_string, mut escaped) = (false, false);
    for byte in body.bytes() {
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

// The last value of each of `keys` in an object, each only scanned, so no number in it is read.
fn last_values<'a, const N: usize>(
    json: &'a str,
    keys: [&'static str; N],
) -> Option<[Option<&'a RawValue>; N]> {
    struct Last<const N: usize>([&'static str; N]);
    impl<'de, const N: usize> Visitor<'de> for Last<N> {
        type Value = [Option<&'de RawValue>; N];

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an object")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut found = [None; N];
            while let Some(key) = map.next_key::<Cow<str>>()? {
                let value = map.next_value::<&RawValue>()?;
                if let Some(slot) = self.0.iter().position(|wanted| *wanted == key) {
                    found[slot] = Some(value);
                }
            }
            Ok(found)
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let found = deserializer.deserialize_map(Last(keys)).ok()?;
    deserializer.end().ok()?;
    Some(found)
}

// A string's text; `None` for any other value.
fn text(value: &RawValue) -> Option<String> {
    serde_json::from_str(value.get()).ok()
}
