//! Layer 2: what a body names as an MCP message, and the endpoint it and a path resolve to.

use serde::Deserialize;
use serde_json::{Value, json};

use super::value::{Bytes, encode_unparsed, keep};
use super::{DEFAULT_BYTE_BUDGET, Outcome, read_cases, run_each};
use crate::body::Unparsed;
use crate::body::mcp::{McpReading, read_endpoint};
use crate::endpoint::{Endpoint, Resolution};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(rename = "note")]
    _note: Option<String>,
    body: Bytes,
    path: String,
    expect: Value,
}

pub(super) fn run_cases(text: &str) -> Result<Vec<Outcome>, String> {
    run_each(read_cases::<Case>(text)?, |case| {
        let got = case.body.to_bytes().and_then(|body| {
            // Within a budget first, since `McpReading::read` takes none: a parser that never
            // stops ends there, not in memory.
            let bounded = keep(&body, DEFAULT_BYTE_BUDGET).map_err(|why| format!("{why:?}"))?;
            if matches!(bounded.json(), Err(Unparsed::PastBudget)) {
                return Err("past the runner's byte budget".into());
            }
            let mcp = McpReading::read(&body);
            Ok(json!({
                "mcp": encode_mcp_reading(&mcp),
                "endpoint": encode_resolution(&read_endpoint(&case.path, &mcp)),
            }))
        });
        (case.id, case.expect, got)
    })
}

fn encode_mcp_reading(mcp: &McpReading) -> Value {
    match mcp {
        McpReading::Read { method, tool } => json!({"read": {"method": method, "tool": tool}}),
        McpReading::Unparsed(why) => json!({"unparsed": encode_unparsed(*why)}),
        McpReading::Batch => json!("batch"),
        McpReading::Ambiguous => json!("ambiguous"),
    }
}

fn encode_resolution(resolution: &Resolution) -> Value {
    match resolution {
        Resolution::Discovery => json!("discovery"),
        Resolution::Judged(Endpoint::Keyless) => json!({"judged": "keyless"}),
        Resolution::Judged(Endpoint::Unrecognised) => json!({"judged": "unrecognised"}),
        Resolution::Judged(Endpoint::Resolved(key)) => {
            json!({"judged": {"resolved": key.as_str()}})
        }
    }
}
