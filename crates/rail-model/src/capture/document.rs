//! The call record document a window becomes, serialize-only, its fields in the field reference's
//! order.

use std::time::SystemTime;

use serde::ser::{Error, SerializeMap};
use serde::{Serialize, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::agent::AgentId;
use crate::redaction::REDACTION_VERSION;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    sequence: Option<Vec<Step>>,
}

/// What the sender gives a document when it queues it: none of it depends on the window.
#[derive(Clone, Copy, Debug)]
pub struct Envelope {
    /// Kept when the document is sent again.
    pub document_id: Uuid,
    pub generated_at: SystemTime,
    /// Whole windows dropped since the last one sent.
    pub windows_dropped: u64,
}

/// Why a window captured nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotCapturing {
    /// Rail Center said there's no plan.
    NoPlan,
    /// A plan of a `schema_version` this reader doesn't know.
    PlanUnknown(PlanNamed),
    /// A plan this reader refuses.
    PlanInvalid(PlanNamed),
    /// A plan redacted by another version of the redaction rule.
    RedactionMismatch(PlanNamed),
}

/// The plan held, as its peek names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanNamed {
    pub plan_hash: String,
    /// Where the peek could read one.
    pub plan_version: Option<u64>,
}

impl Document {
    /// A window that captured nothing, and why.
    pub fn not_capturing(
        why: NotCapturing,
        agent: AgentId,
        start: SystemTime,
        end: SystemTime,
        envelope: Envelope,
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
        Self::new(status, captured, envelope)
    }

    pub(crate) fn capturing(captured: Captured, envelope: Envelope) -> Self {
        Self::new(Status::Capturing, captured, envelope)
    }

    fn new(status: Status, captured: Captured, envelope: Envelope) -> Self {
        let truncated = Truncated {
            windows_dropped: envelope.windows_dropped,
            ..captured.truncated
        };
        Self {
            schema_version: SCHEMA_VERSION,
            generated_at: Timestamp(envelope.generated_at),
            document_id: envelope.document_id,
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

/// What a window's document holds but its envelope and status.
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
    pub(crate) body_unread: u64,
    pub(crate) windows_dropped: u64,
    pub(crate) path_unresolved: u64,
    pub(crate) values_replaced: u64,
    pub(crate) values_ambiguous: u64,
    /// Present exactly when the document has a sequence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) steps_dropped: Option<u64>,
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
    pub(crate) context_keys: Fields<Option<String>>,
    #[serde(skip_serializing_if = "Fields::is_empty")]
    pub(crate) field_types: Fields<String>,
    pub(crate) measures: Fields<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Step {
    pub(crate) at: Timestamp,
    pub(crate) record_id: String,
}

/// An object's fields in the plan's order.
#[derive(Debug, Default)]
pub(crate) struct Fields<T>(pub(crate) Vec<(String, T)>);

impl<T> Fields<T> {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T: Serialize> Serialize for Fields<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in &self.0 {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

/// RFC 3339 in UTC, `Z`, whole seconds.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timestamp(pub(crate) SystemTime);

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let written = OffsetDateTime::from(self.0)
            .replace_nanosecond(0)
            .map_err(S::Error::custom)?
            .format(&Rfc3339)
            .map_err(S::Error::custom)?;
        serializer.serialize_str(&written)
    }
}
