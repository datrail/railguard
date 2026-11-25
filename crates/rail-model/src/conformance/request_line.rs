//! Layer 1: a call's request line, as sent, and every reading of it.

use serde::Deserialize;
use serde_json::{Value, json};

use super::value::{Bytes, encode_host_reading, encode_reported_method, encode_unmatchable};
use super::{Outcome, read_cases, run_each};
use crate::request_line::RequestLine;

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
    run_each(read_cases::<Case>(text)?, |case| {
        let got = run_case(&case);
        (case.id, case.expect, got)
    })
}

fn run_case(case: &Case) -> Result<Value, String> {
    let line = RequestLine::read(
        &case.authority.to_bytes()?,
        &case.method.to_bytes()?,
        &case.path.to_bytes()?,
    );
    let segments = |read: fn(&RequestLine, usize) -> Option<&str>| {
        (0..)
            .map_while(|index| read(&line, index))
            .collect::<Vec<_>>()
    };
    Ok(json!({
        "host": encode_host_reading(line.host_reading()),
        "method": line.method(),
        "reported_method": encode_reported_method(line.reported_method()),
        "path": line.path(),
        "redacted_path": line.redacted_path(),
        "segments": segments(RequestLine::path_segment),
        "redacted_segments": segments(RequestLine::redacted_path_segment),
        "query": line.query(),
        "unmatchable": line.unmatchable().map(encode_unmatchable),
    }))
}
