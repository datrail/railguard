//! Layer 2: what a body names as an MCP message, and the endpoint it and a path resolve to.

use serde::Deserialize;
use serde_json::{Value, json};

use super::value::{Bytes, encode_unparsed};
use super::{Outcome, create_outcome, read_cases};
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
    read_cases::<Case>(text)?
        .into_iter()
        .map(|case| {
            let mcp = McpReading::read(&case.body.to_bytes()?);
            let got = json!({
                "mcp": encode_mcp_reading(&mcp),
                "endpoint": encode_resolution(&read_endpoint(&case.path, &mcp)),
            });
            Ok(create_outcome(&case.id, case.expect, got))
        })
        .collect()
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
