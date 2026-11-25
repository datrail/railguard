//! The vectors' encoding of this reader's values: a variant holding nothing as its snake-case
//! name, one holding something as a one-key object of it.

use std::borrow::Cow;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::body::{BodyBuffer, ByteBudget, RequestBody, Unparsed, Unread};
use crate::condition::{BodyUnread, Items, Lookup, Refusal, Verdict};
use crate::request_line::{HostReading, ReportedMethod, Unmatchable};

/// Bytes as a vector writes them: text, or `{"hex": …}` for bytes that aren't UTF-8.
#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub(super) enum Bytes {
    Text(String),
    Hex { hex: String },
}

impl Bytes {
    pub(super) fn to_bytes(&self) -> Result<Vec<u8>, String> {
        match self {
            Self::Text(text) => Ok(text.as_bytes().to_vec()),
            Self::Hex { hex } => decode_hex(hex),
        }
    }
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    let digits = hex.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return Err(format!("hex {hex:?}: an odd number of digits"));
    }
    digits
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| format!("hex {hex:?}: not hex"))
        })
        .collect()
}

pub(super) fn encode_lookup(lookup: &Lookup) -> Value {
    match lookup {
        Lookup::Absent => json!("absent"),
        Lookup::String(text) => json!({"string": text}),
        Lookup::Number(number) => json!({"number": encode_number(*number)}),
        Lookup::Bool(value) => json!({"bool": value}),
        Lookup::List(items) => {
            json!({"list": items.iter().map(|item| encode_lookup(&item)).collect::<Vec<_>>()})
        }
        Lookup::Object => json!("object"),
        Lookup::Ambiguous => json!("ambiguous"),
        Lookup::BodyUnread(why) => json!({"body_unread": encode_body_unread(*why)}),
    }
}

// Past a double as a body's number saturates; a whole number as one, so `1` and `1.0` are one.
fn encode_number(number: f64) -> Value {
    match number {
        f64::INFINITY => json!("inf"),
        f64::NEG_INFINITY => json!("-inf"),
        whole if whole.fract() == 0.0 && whole.abs() < 2f64.powi(53) && whole != 0.0 => {
            json!(whole as i64)
        }
        zero if zero == 0.0 && zero.is_sign_positive() => json!(0),
        number => json!(number),
    }
}

pub(super) fn encode_body_unread(why: BodyUnread) -> Value {
    match why {
        BodyUnread::Unread(why) => json!({"unread": encode_unread(why)}),
        BodyUnread::Unparsed(why) => json!({"unparsed": encode_unparsed(why)}),
        BodyUnread::Batch => json!("batch"),
    }
}

fn encode_unread(why: Unread) -> &'static str {
    match why {
        Unread::PastCap => "past_cap",
        Unread::PastBudget => "past_budget",
        Unread::NotSent => "not_sent",
        Unread::CutShort => "cut_short",
    }
}

pub(super) fn encode_unparsed(why: Unparsed) -> &'static str {
    match why {
        Unparsed::Malformed => "malformed",
        Unparsed::PastBudget => "past_budget",
    }
}

pub(super) fn encode_refusal(refusal: Refusal) -> Value {
    json!({"refused": refusal})
}

pub(super) fn encode_verdict(verdict: Verdict) -> Value {
    match verdict {
        Verdict::Holds => json!("holds"),
        Verdict::DoesNotHold => json!("does_not_hold"),
        Verdict::Ambiguous => json!("ambiguous"),
        Verdict::BodyUnread(why) => json!({"body_unread": encode_body_unread(why)}),
        Verdict::Refused(refusal) => encode_refusal(refusal),
    }
}

pub(super) fn encode_host_reading(host: &HostReading) -> Value {
    match host {
        HostReading::Read(host) => json!({"read": host}),
        HostReading::Refused => json!("refused"),
        HostReading::PastLongest => json!("past_longest"),
    }
}

pub(super) fn encode_reported_method(method: ReportedMethod) -> Value {
    match method {
        ReportedMethod::Standard(method) => json!(method),
        ReportedMethod::Other => json!("other"),
    }
}

pub(super) fn encode_unmatchable(why: Unmatchable) -> &'static str {
    match why {
        Unmatchable::NotUtf8 => "not_utf8",
        Unmatchable::Bracket => "bracket",
        Unmatchable::EncodedSlash => "encoded_slash",
        Unmatchable::DotSegment => "dot_segment",
        Unmatchable::EmptySegment => "empty_segment",
    }
}

pub(super) fn decode_unread(name: &str) -> Option<Unread> {
    match name {
        "past_cap" => Some(Unread::PastCap),
        "past_budget" => Some(Unread::PastBudget),
        "not_sent" => Some(Unread::NotSent),
        "cut_short" => Some(Unread::CutShort),
        _ => None,
    }
}

/// `bytes` kept whole, within a budget shared with its tree.
pub(super) fn keep(bytes: &[u8], budget_bytes: usize) -> Result<RequestBody, Unread> {
    let mut buffer = BodyBuffer::new(usize::MAX, &ByteBudget::new(budget_bytes));
    buffer.append(bytes);
    buffer.finish().map_err(Unread::from)
}

/// A value as a vector writes it, decoded and held, for a `Lookup` to borrow.
pub(super) enum HeldValue {
    Absent,
    String(String),
    Number(f64),
    Bool(bool),
    /// Its items written as a body and read as one, so an item reads as a body's does.
    List(RequestBody),
    Object,
    Ambiguous,
    BodyUnread(BodyUnread),
}

impl HeldValue {
    pub(super) fn decode(value: &Value) -> Result<Self, String> {
        Ok(match value {
            Value::String(name) if name == "absent" => Self::Absent,
            Value::String(name) if name == "object" => Self::Object,
            Value::String(name) if name == "ambiguous" => Self::Ambiguous,
            Value::Object(fields) if fields.len() == 1 => {
                let (kind, inner) = fields.iter().next().expect("one field");
                match (kind.as_str(), inner) {
                    ("string", Value::String(text)) => Self::String(text.clone()),
                    ("number", number) => Self::Number(decode_number(number)?),
                    ("bool", Value::Bool(value)) => Self::Bool(*value),
                    ("list", Value::Array(_)) => {
                        let mut text = String::new();
                        write_body_text(value, &mut text)?;
                        Self::List(
                            keep(text.as_bytes(), usize::MAX).map_err(|why| format!("{why:?}"))?,
                        )
                    }
                    ("body_unread", why) => Self::BodyUnread(decode_body_unread(why)?),
                    _ => return Err(format!("not a value: {value}")),
                }
            }
            _ => return Err(format!("not a value: {value}")),
        })
    }

    pub(super) fn lookup(&self) -> Lookup<'_> {
        match self {
            Self::Absent => Lookup::Absent,
            Self::String(text) => Lookup::String(Cow::Borrowed(text)),
            Self::Number(number) => Lookup::Number(*number),
            Self::Bool(value) => Lookup::Bool(*value),
            Self::List(body) => {
                Lookup::List(Items::Body(body.json().expect("a list written as JSON")))
            }
            Self::Object => Lookup::Object,
            Self::Ambiguous => Lookup::Ambiguous,
            Self::BodyUnread(why) => Lookup::BodyUnread(*why),
        }
    }
}

fn decode_number(number: &Value) -> Result<f64, String> {
    match number {
        Value::String(text) if text == "inf" => Ok(f64::INFINITY),
        Value::String(text) if text == "-inf" => Ok(f64::NEG_INFINITY),
        number => number
            .as_f64()
            .ok_or_else(|| format!("not a number: {number}")),
    }
}

fn decode_body_unread(why: &Value) -> Result<BodyUnread, String> {
    let unparsed = |name: &str| match name {
        "malformed" => Some(Unparsed::Malformed),
        "past_budget" => Some(Unparsed::PastBudget),
        _ => None,
    };
    let decoded = match why {
        Value::String(name) if name == "batch" => Some(BodyUnread::Batch),
        Value::Object(fields) if fields.len() == 1 => match fields.iter().next() {
            Some((kind, Value::String(name))) if kind == "unread" => {
                decode_unread(name).map(BodyUnread::Unread)
            }
            Some((kind, Value::String(name))) if kind == "unparsed" => {
                unparsed(name).map(BodyUnread::Unparsed)
            }
            _ => None,
        },
        _ => None,
    };
    decoded.ok_or_else(|| format!("not a reason a body is unread: {why}"))
}

// A value as a body would write it: a number past a double as one, a value that can't be told as
// a string that doesn't decode.
fn write_body_text(value: &Value, text: &mut String) -> Result<(), String> {
    match value {
        Value::String(name) if name == "absent" => text.push_str("null"),
        Value::String(name) if name == "object" => text.push_str("{}"),
        Value::String(name) if name == "ambiguous" => text.push_str(r#""\ud800""#),
        Value::Object(fields) if fields.len() == 1 => {
            let (kind, inner) = fields.iter().next().expect("one field");
            match (kind.as_str(), inner) {
                ("string", Value::String(inner)) => {
                    text.push_str(&Value::from(inner.as_str()).to_string())
                }
                ("number", Value::String(inf)) if inf == "inf" => text.push_str("1e400"),
                ("number", Value::String(inf)) if inf == "-inf" => text.push_str("-1e400"),
                ("number", number @ Value::Number(_)) => text.push_str(&number.to_string()),
                ("bool", Value::Bool(inner)) => {
                    text.push_str(if *inner { "true" } else { "false" })
                }
                ("list", Value::Array(items)) => {
                    text.push('[');
                    for (index, item) in items.iter().enumerate() {
                        if index > 0 {
                            text.push(',');
                        }
                        write_body_text(item, text)?;
                    }
                    text.push(']');
                }
                _ => return Err(format!("not a list's item: {value}")),
            }
        }
        _ => return Err(format!("not a list's item: {value}")),
    }
    Ok(())
}
