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
    status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_hash: Option<String>,
    redaction_version: u64,
    window: Span,
    truncated: Truncated,
    hosts: Vec<HostTotal>,
    records: Vec<CallRecord>,
    /// A capturing window's, empty under a plan asking for no steps; none for one that captured
    /// nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    sequence: Option<Vec<Step>>,
}

/// Why a window captured nothing.
#[derive(Debug)]
pub enum NotCapturing {
    /// Rail Center said there's no plan.
    NoPlan,
    /// A plan of a `schema_version` this reader doesn't know.
    PlanUnknown(PlanId),
    /// A plan this reader refuses.
    PlanInvalid(PlanId),
    /// A plan redacted by another version of the redaction rule.
    RedactionMismatch(PlanId),
}

/// The plan held, as its peek names it.
#[derive(Debug)]
pub struct PlanId {
    pub plan_hash: String,
    /// Where the peek could read one.
    pub plan_version: Option<u64>,
}

impl Document {
    /// A window that captured nothing, and why; given what a capturing window's document is given
    /// in [`ClosedWindow::into_document`](super::ClosedWindow::into_document).
    pub fn not_capturing(
        why: NotCapturing,
        agent: AgentId,
        start: SystemTime,
        end: SystemTime,
        document_id: Uuid,
        generated_at: SystemTime,
        windows_dropped: u64,
    ) -> Self {
        let (status, plan) = match why {
            NotCapturing::NoPlan => (Status::NoPlan, None),
            NotCapturing::PlanUnknown(plan) => (Status::PlanUnknown, Some(plan)),
            NotCapturing::PlanInvalid(plan) => (Status::PlanInvalid, Some(plan)),
            NotCapturing::RedactionMismatch(plan) => (Status::RedactionMismatch, Some(plan)),
        };
        let captured = Captured {
            agent,
            plan_version: plan.as_ref().and_then(|plan| plan.plan_version),
            plan_hash: plan.map(|plan| plan.plan_hash),
            start,
            end,
            truncated: Truncated::default(),
            hosts: Vec::new(),
            records: Vec::new(),
            sequence: None,
        };
        Self::new(status, captured, document_id, generated_at, windows_dropped)
    }

    pub(crate) fn capturing(
        captured: Captured,
        document_id: Uuid,
        generated_at: SystemTime,
        windows_dropped: u64,
    ) -> Self {
        let status = Status::Capturing;
        Self::new(status, captured, document_id, generated_at, windows_dropped)
    }

    fn new(
        status: Status,
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
            status,
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

/// What a window's document holds but what its sender gives and its status.
#[derive(Debug)]
pub(crate) struct Captured {
    pub(crate) agent: AgentId,
    pub(crate) plan_version: Option<u64>,
    pub(crate) plan_hash: Option<String>,
    pub(crate) start: SystemTime,
    pub(crate) end: SystemTime,
    pub(crate) truncated: Truncated,
    pub(crate) hosts: Vec<HostTotal>,
    pub(crate) records: Vec<CallRecord>,
    pub(crate) sequence: Option<Vec<Step>>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Capturing,
    PlanUnknown,
    PlanInvalid,
    RedactionMismatch,
    NoPlan,
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
    /// Present exactly when the document has a sequence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) steps_dropped: Option<u64>,
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
