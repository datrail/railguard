//! What an MCP request body names: its method and tool, read as the Python gateway reads them.

use std::borrow::Cow;
use std::fmt;

use rail_policy::{Endpoint, Resolution, resolve_endpoint};
use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, de};

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
    let Ok(Message { method, name }) = serde_json::from_str(body) else {
        return Resolution::Judged(Endpoint::Unrecognised);
    };
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

// A key repeated keeps its last value, and a value of another type reads as absent, as in Python.
struct Message<'a> {
    method: Option<Cow<'a, str>>,
    name: Option<Cow<'a, str>>,
}

impl<'de> Deserialize<'de> for Message<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Root;
        impl<'de> Visitor<'de> for Root {
            type Value = Message<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut message = Message {
                    method: None,
                    name: None,
                };
                while let Some(key) = map.next_key::<Cow<str>>()? {
                    match &*key {
                        "method" => message.method = map.next_value_seed(StringAt(None))?,
                        "params" => message.name = map.next_value_seed(StringAt(Some("name")))?,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(message)
            }
        }
        deserializer.deserialize_map(Root)
    }
}

// The string a value holds at a key, or is itself with no key; `None` for anything else.
#[derive(Clone, Copy)]
struct StringAt(Option<&'static str>);

impl<'de> DeserializeSeed<'de> for StringAt {
    type Value = Option<Cow<'de, str>>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for StringAt {
    type Value = Option<Cow<'de, str>>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any value")
    }

    fn visit_borrowed_str<E: de::Error>(self, text: &'de str) -> Result<Self::Value, E> {
        Ok(self.0.is_none().then_some(Cow::Borrowed(text)))
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
        Ok(self.0.is_none().then(|| Cow::Owned(text.to_owned())))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut found = None;
        while let Some(key) = map.next_key::<Cow<str>>()? {
            if self.0 == Some(&*key) {
                found = map.next_value_seed(StringAt(None))?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(found)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        IgnoredAny.visit_seq(seq).map(|_| None)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
}
