//! The call record document a window becomes, serialize-only, its fields in the field reference's
//! order.

use std::time::SystemTime;

use indexmap::IndexMap;
use serde::Serialize;
use uuid::Uuid;

use crate::agent::AgentId;
use crate::redaction::REDACTION_VERSION;
use crate::timestamp::Timestamp;

const SCHEMA_VERSION: &str = "1.0";

/// One window's document, as sent to Rail Center's `POST /v1/call-records`.
#[derive(Debug, Serialize)]
pub struct Document {
    schema_version: &'static str,
    generated_at: Timestamp,
    document_id: Uuid,
    agent: AgentId,
    plan_version: u64,
    plan_hash: String,
    redaction_version: u64,
    window: Span,
    truncated: Truncated,
    hosts: Vec<HostTotal>,
    records: Vec<CallRecord>,
    /// Empty under a plan asking for no steps.
    sequence: Vec<Step>,
}

impl Document {
    pub(crate) fn new(
        captured: Captured,
        document_id: Uuid,
        generated_at: SystemTime,
        windows_dropped: u64,
    ) -> Self {
        let truncated = Truncated {
            windows_dropped,
            ..captured.truncated
        };
        Self {
            schema_version: SCHEMA_VERSION,
            generated_at: Timestamp(generated_at),
            document_id,
            agent: captured.agent,
            plan_version: captured.plan_version,
            plan_hash: captured.plan_hash,
            redaction_version: REDACTION_VERSION,
            window: Span::new(captured.start, captured.end),
            truncated,
            hosts: captured.hosts,
            records: captured.records,
            sequence: captured.sequence,
        }
    }
}

/// What a window's document holds but what its sender gives.
#[derive(Debug)]
pub(crate) struct Captured {
    pub(crate) agent: AgentId,
    pub(crate) plan_version: u64,
    pub(crate) plan_hash: String,
    pub(crate) max_unsent_windows: u32,
    pub(crate) start: SystemTime,
    pub(crate) end: SystemTime,
    pub(crate) truncated: Truncated,
    pub(crate) hosts: Vec<HostTotal>,
    pub(crate) records: Vec<CallRecord>,
    pub(crate) sequence: Vec<Step>,
}

#[derive(Debug, Serialize)]
struct Span {
    start: Timestamp,
    end: Timestamp,
}

impl Span {
    fn new(start: SystemTime, end: SystemTime) -> Self {
        Self {
            start: Timestamp(start),
            end: Timestamp(end),
        }
    }
}

/// Every count that applies, 0 when nothing was lost.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Truncated {
    pub(crate) calls_coarsened: u64,
    pub(crate) calls_dropped: u64,
    pub(crate) entries_skipped: u64,
    pub(crate) body_unread: BodyUnreadCounts,
    pub(crate) windows_dropped: u64,
    pub(crate) path_unresolved: u64,
    pub(crate) values_replaced: u64,
    pub(crate) values_ambiguous: u64,
    pub(crate) steps_dropped: u64,
}

/// The calls an entry could have taken whose body's values weren't told, by why.
#[derive(Debug, Default, Serialize)]
pub(crate) struct BodyUnreadCounts {
    pub(crate) past_cap: u64,
    pub(crate) past_budget: u64,
    pub(crate) not_sent: u64,
    pub(crate) cut_short: u64,
    pub(crate) not_json: u64,
    pub(crate) batch: u64,
    pub(crate) ambiguous: u64,
    pub(crate) step_budget: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct HostTotal {
    pub(crate) host: String,
    pub(crate) count: u64,
    /// 0 until the contract defines an error and the callout sees responses.
    pub(crate) error_count: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct CallRecord {
    pub(crate) record_id: String,
    pub(crate) entry_id: String,
    /// Each in the plan's order.
    pub(crate) context_keys: IndexMap<String, Option<String>>,
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub(crate) field_types: IndexMap<String, String>,
    pub(crate) measures: IndexMap<String, u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Step {
    pub(crate) at: Timestamp,
    pub(crate) record_id: String,
}
