//! The vectors' encoding of this reader's values: a variant holding nothing as its snake-case
//! name, one holding something as a one-key object of it.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::body::{Unparsed, Unread};
use crate::condition::{BodyUnread, Lookup, Refusal, Verdict};
use crate::request_line::HostReading;

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

// Past a double, as a body's number saturates.
pub(super) fn encode_number(number: f64) -> Value {
    match number {
        f64::INFINITY => json!("inf"),
        f64::NEG_INFINITY => json!("-inf"),
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

pub(super) fn encode_unread(why: Unread) -> &'static str {
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
