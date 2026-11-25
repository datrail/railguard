//! Layer 1: a call's request line, as sent, and every reading of it.

use serde::Deserialize;
use serde_json::{Value, json};

use super::value::{Bytes, encode_host_reading};
use super::{Outcome, create_outcome, read_cases};
use crate::request_line::{ReportedMethod, RequestLine, Unmatchable};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(rename = "note")]
    _note: Option<String>,
    authority: Bytes,
    method: Bytes,
    path: Bytes,
    expect: Value,
}

pub(super) fn run_cases(text: &str) -> Result<Vec<Outcome>, String> {
    read_cases::<Case>(text)?
        .into_iter()
        .map(|case| {
            let line = RequestLine::read(
                &case.authority.to_bytes()?,
                &case.method.to_bytes()?,
                &case.path.to_bytes()?,
            );
            Ok(create_outcome(
                &case.id,
                case.expect,
                encode_request_line(&line),
            ))
        })
        .collect()
}

pub(super) fn encode_request_line(line: &RequestLine) -> Value {
    let segments = |read: fn(&RequestLine, usize) -> Option<&str>| {
        (0..)
            .map_while(|index| read(line, index))
            .collect::<Vec<_>>()
    };
    json!({
        "host": encode_host_reading(line.host_reading()),
        "method": line.method(),
        "reported_method": encode_reported_method(line.reported_method()),
        "path": line.path(),
        "redacted_path": line.redacted_path(),
        "segments": segments(RequestLine::path_segment),
        "redacted_segments": segments(RequestLine::redacted_path_segment),
        "query": line.query(),
        "unmatchable": line.unmatchable().map(encode_unmatchable),
    })
}

pub(super) fn encode_reported_method(method: ReportedMethod) -> Value {
    match method {
        ReportedMethod::Standard(method) => json!(method),
        ReportedMethod::Other => json!("other"),
    }
}

fn encode_unmatchable(why: Unmatchable) -> &'static str {
    match why {
        Unmatchable::NotUtf8 => "not_utf8",
        Unmatchable::Bracket => "bracket",
        Unmatchable::EncodedSlash => "encoded_slash",
        Unmatchable::DotSegment => "dot_segment",
        Unmatchable::EmptySegment => "empty_segment",
    }
}
