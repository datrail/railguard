//! A window: the calls captured between two times, grouped into call records, with their hosts'
//! totals and their order, closed into a document.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use uuid::Uuid;

use super::call::{
    CallReader, KeptValue, KeyReading, MeasureReading, ObservedCall, PathReading, RecordReading,
    Taken, Untaken,
};
use super::compile::CompiledPlan;
use super::document::{CallRecord, Captured, Document, Fields, HostTotal, Sent, Step, Truncated};
use super::plan::{ContextKey, Record};
use crate::body::{Unparsed, Unread};
use crate::condition::BodyUnread;
use crate::redaction::REDACTED;
use crate::request_line::{HostReading, ReportedMethod, RequestLine};
use crate::timestamp::Timestamp;

// What a value that can't be told is written as.
const AMBIGUOUS: &str = "[ambiguous]";

// The host of the catch-all's record past the group ceiling, and of a host first seen in a call
// coarsened there.
const OVERFLOW: &str = "[overflow]";

/// Why a call an entry could have taken went to the catch-all with its body's values not told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyUnreadReason {
    PastCap,
    /// The body, or its tree, past the byte budget.
    PastBudget,
    NotSent,
    CutShort,
    NotJson,
    Batch,
    Ambiguous,
    /// The walk spent the step budget on it.
    StepBudget,
}

/// The calls captured since a window opened, under one plan.
#[derive(Debug)]
pub struct Window {
    plan: Arc<CompiledPlan>,
    start: SystemTime,
    /// In the order opened.
    records: Vec<CallRecord>,
    /// Each record's index, by what tells it apart.
    groups: HashMap<GroupKey, usize>,
    overflow: Option<usize>,
    /// In the order first seen.
    hosts: Vec<HostTotal>,
    host_index: HashMap<String, usize>,
    /// With the plan's `max_steps_per_window`, that many: when each call was received, and its
    /// record.
    steps: Option<(usize, Vec<(SystemTime, usize)>)>,
    /// The latest time a call was received, from the window's start: a clock stepped back
    /// mustn't write steps out of order.
    latest: SystemTime,
    truncated: Truncated,
}

// The record's id, an entry's or the catch-all's, unique in the plan, and its context keys' values.
#[derive(Debug, PartialEq, Eq, Hash)]
struct GroupKey {
    record_id: String,
    values: Vec<Option<String>>,
}

// What a record writes of one call, and what it lost.
#[derive(Default)]
struct Written {
    measures: Vec<u64>,
    path_unresolved: bool,
    values_replaced: u64,
    ambiguous: bool,
}

impl Window {
    pub fn open(plan: Arc<CompiledPlan>, start: SystemTime) -> Self {
        let steps = plan
            .plan()
            .budget
            .max_steps_per_window
            .map(|max| (max as usize, Vec::new()));
        let truncated = Truncated {
            entries_skipped: plan.entries_skipped() as u64,
            steps_dropped: steps.as_ref().map(|_| 0),
            ..Truncated::default()
        };
        Self {
            plan,
            start,
            records: Vec::new(),
            groups: HashMap::new(),
            overflow: None,
            hosts: Vec::new(),
            host_index: HashMap::new(),
            steps,
            latest: start,
            truncated,
        }
    }

    /// Counts `call`, received at `at`; why its body's values weren't told, if an entry could have
    /// taken it.
    pub fn record(&mut self, call: &ObservedCall, at: SystemTime) -> Option<BodyUnreadReason> {
        let plan = Arc::clone(&self.plan);
        let request_line = RequestLine::read(&call.authority, &call.method, &call.path);
        let mut reader = CallReader::new(&request_line, call);
        let taken = reader.take(&plan);
        let reason = body_unread_reason(&taken);
        self.truncated.body_unread += u64::from(reason.is_some());
        let catch_all = &plan.plan().catch_all;
        let (record_id, mut spec) = match &taken {
            Taken::Entry(entry) => (&entry.id, &entry.record),
            Taken::CatchAll(catch_all, _) => (&catch_all.id, &catch_all.record),
        };
        let (values, mut written) = write(spec, &reader.read(spec), &request_line);
        let mut key = GroupKey {
            record_id: record_id.clone(),
            values,
        };
        let at_ceiling = self.records.len() >= plan.plan().budget.max_groups_per_window as usize;
        // Past the ceiling, an entry's call is the catch-all's, and a new catch-all group the
        // overflow's.
        let coarsened = at_ceiling && !self.groups.contains_key(&key);
        if coarsened && matches!(taken, Taken::Entry(_)) {
            spec = &catch_all.record;
            let values;
            (values, written) = write(spec, &reader.read(spec), &request_line);
            key = GroupKey {
                record_id: catch_all.id.clone(),
                values,
            };
        }
        let index = match self.groups.get(&key) {
            Some(&index) => index,
            None if !coarsened => self.open_record(key, spec),
            None => {
                // Its keys aren't written, so neither is what they lost.
                written = Written {
                    measures: written.measures,
                    ..Written::default()
                };
                self.overflow()
            }
        };
        self.truncated.calls_coarsened += u64::from(coarsened);
        self.count_host(request_line.host(), coarsened);
        self.add(index, written);
        self.latest = self.latest.max(at);
        match &mut self.steps {
            Some((max, steps)) if steps.len() < *max => steps.push((self.latest, index)),
            Some(_) => *self.truncated.steps_dropped.get_or_insert(0) += 1,
            None => {}
        }
        reason
    }

    /// Counts calls lost before reaching the window.
    pub fn count_dropped(&mut self, calls: u64) {
        self.truncated.calls_dropped = self.truncated.calls_dropped.saturating_add(calls);
    }

    /// The window's document, for the sender to give its envelope; it ends no earlier than its
    /// last call.
    pub fn close(self, end: SystemTime) -> ClosedWindow {
        let plan = self.plan.plan();
        let end = end.max(self.latest);
        let sequence = self.steps.map(|(_, steps)| {
            steps
                .into_iter()
                .map(|(at, index)| Step {
                    at: Timestamp(at),
                    record_id: record_id(index),
                })
                .collect()
        });
        ClosedWindow(Captured {
            agent: plan.agent.clone(),
            plan_version: Some(plan.plan_version),
            plan_hash: Some(plan.content_hash.clone()),
            start: self.start,
            end,
            truncated: self.truncated,
            hosts: self.hosts,
            records: self.records,
            sequence,
        })
    }

    // A host first seen in a coarsened call is totalled under the overflow marker.
    fn count_host(&mut self, host: &str, coarsened: bool) {
        let host = match self.host_index.get(host) {
            None if coarsened => OVERFLOW,
            _ => host,
        };
        let index = *self.host_index.entry(host.to_owned()).or_insert_with(|| {
            self.hosts.push(HostTotal {
                host: host.to_owned(),
                count: 0,
                error_count: 0,
            });
            self.hosts.len() - 1
        });
        self.hosts[index].count += 1;
    }

    fn open_record(&mut self, key: GroupKey, spec: &Record) -> usize {
        let context_keys = spec
            .context_keys
            .iter()
            .zip(&key.values)
            .map(|(spec, value)| (spec.name().to_owned(), value.clone()))
            .collect();
        let field_types = spec
            .context_keys
            .iter()
            .filter_map(|key| match key {
                ContextKey::Named {
                    name, field_type, ..
                } => Some((name.clone(), field_type.clone())),
                _ => None,
            })
            .collect();
        let index = self.push_record(key.record_id.clone(), context_keys, field_types, spec);
        self.groups.insert(key, index);
        index
    }

    // The catch-all's one record past the ceiling, keeping only the host, as the overflow marker.
    fn overflow(&mut self) -> usize {
        if let Some(index) = self.overflow {
            return index;
        }
        let plan = Arc::clone(&self.plan);
        let catch_all = &plan.plan().catch_all;
        let host = vec![("host".to_owned(), Some(OVERFLOW.to_owned()))];
        let index = self.push_record(catch_all.id.clone(), host, Vec::new(), &catch_all.record);
        self.overflow = Some(index);
        index
    }

    fn push_record(
        &mut self,
        entry_id: String,
        context_keys: Vec<(String, Option<String>)>,
        field_types: Vec<(String, String)>,
        spec: &Record,
    ) -> usize {
        let measures = spec
            .measures
            .iter()
            .map(|measure| (measure.name().to_owned(), 0))
            .collect();
        self.records.push(CallRecord {
            record_id: record_id(self.records.len()),
            entry_id,
            context_keys: Fields(context_keys),
            field_types: Fields(field_types),
            measures: Fields(measures),
        });
        self.records.len() - 1
    }

    fn add(&mut self, index: usize, written: Written) {
        let measures = &mut self.records[index].measures.0;
        for ((_, total), bytes) in measures.iter_mut().zip(written.measures) {
            *total = total.saturating_add(bytes);
        }
        self.truncated.path_unresolved += u64::from(written.path_unresolved);
        self.truncated.values_replaced += written.values_replaced;
        self.truncated.values_ambiguous += u64::from(written.ambiguous);
    }
}

/// A closed window, waiting for its envelope.
#[derive(Debug)]
pub struct ClosedWindow(Captured);

impl ClosedWindow {
    /// Given its id, kept when it's sent again; when it was built; and the windows dropped since
    /// the last one sent.
    pub fn into_document(
        self,
        document_id: Uuid,
        generated_at: SystemTime,
        windows_dropped: u64,
    ) -> Document {
        let sent = Sent {
            document_id,
            generated_at,
            windows_dropped,
        };
        Document::capturing(self.0, sent)
    }
}

fn record_id(index: usize) -> String {
    format!("r{}", index + 1)
}

fn body_unread_reason(taken: &Taken) -> Option<BodyUnreadReason> {
    let Taken::CatchAll(_, why) = taken else {
        return None;
    };
    Some(match why {
        Untaken::BodyUnread(_, why) => match why {
            BodyUnread::Unread(Unread::PastCap) => BodyUnreadReason::PastCap,
            BodyUnread::Unread(Unread::PastBudget) => BodyUnreadReason::PastBudget,
            BodyUnread::Unread(Unread::NotSent) => BodyUnreadReason::NotSent,
            BodyUnread::Unread(Unread::CutShort) => BodyUnreadReason::CutShort,
            BodyUnread::Unparsed(Unparsed::Malformed) => BodyUnreadReason::NotJson,
            BodyUnread::Unparsed(Unparsed::PastBudget) => BodyUnreadReason::PastBudget,
            BodyUnread::Batch => BodyUnreadReason::Batch,
        },
        Untaken::Ambiguous(_) => BodyUnreadReason::Ambiguous,
        Untaken::Stopped(..) => BodyUnreadReason::StepBudget,
        Untaken::NoEntry | Untaken::Unmatchable(_) | Untaken::Refused(..) => return None,
    })
}

// Its context keys' values, and its measures with what they lost; a built-in key has no path, so
// it never counts as unresolved.
fn write(
    spec: &Record,
    record: &RecordReading,
    request_line: &RequestLine,
) -> (Vec<Option<String>>, Written) {
    debug_assert_eq!(spec.context_keys.len(), record.context_keys.len());
    let mut written = Written::default();
    let values = record
        .context_keys
        .iter()
        .map(|reading| match reading {
            KeyReading::Method(method) => {
                written.values_replaced += u64::from(*method == ReportedMethod::Other);
                Some(method.as_str().to_owned())
            }
            KeyReading::Host(host) => {
                written.values_replaced += u64::from(*host == HostReading::PastLongest);
                Some(request_line.host().to_owned())
            }
            KeyReading::PathPrefix(prefix) => Some(prefix.clone()),
            KeyReading::Operation(reading) => write_value(reading, &mut written, false),
            KeyReading::Path(reading) => write_value(reading, &mut written, true),
        })
        .collect();
    written.measures = record
        .measures
        .iter()
        .map(|reading| match reading {
            MeasureReading::CallCount => 1,
            MeasureReading::ErrorCount => 0,
            MeasureReading::RequestBytes(bytes) => *bytes,
            MeasureReading::SizeInBytes(paths) => paths
                .iter()
                .map(|path| match path {
                    PathReading::Found(bytes) => *bytes,
                    PathReading::Ambiguous => {
                        written.ambiguous = true;
                        0
                    }
                    PathReading::Absent | PathReading::BodyUnread(_) | PathReading::Refused(_) => {
                        written.path_unresolved = true;
                        0
                    }
                })
                .sum(),
        })
        .collect();
    (values, written)
}

// A value found, kept or replaced; `null` for nothing found or a value a key doesn't keep, which
// a plan path counts as unresolved.
fn write_value(
    reading: &PathReading<KeptValue>,
    written: &mut Written,
    from_plan_path: bool,
) -> Option<String> {
    match reading {
        PathReading::Found(KeptValue::String(text)) => Some(text.clone()),
        PathReading::Found(KeptValue::Replaced(_)) => {
            written.values_replaced += 1;
            Some(REDACTED.to_owned())
        }
        PathReading::Ambiguous => {
            written.ambiguous = true;
            Some(AMBIGUOUS.to_owned())
        }
        PathReading::Found(KeptValue::NotAString)
        | PathReading::Absent
        | PathReading::BodyUnread(_)
        | PathReading::Refused(_) => {
            written.path_unresolved |= from_plan_path;
            None
        }
    }
}
