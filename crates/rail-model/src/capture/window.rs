//! A window: the calls captured between two times, grouped into call records, with their hosts'
//! totals and their order, closed into a document.

use std::collections::HashMap;
use std::time::SystemTime;

use super::call::{
    CapturedCall, KeptValue, KeyReading, MeasureReading, ObservedCall, PathReading, RecordReading,
    Taken, Untaken,
};
use super::compile::CompiledPlan;
use super::document::{
    CallRecord, Captured, Document, Envelope, Fields, HostTotal, Step, Timestamp, Truncated,
};
use super::plan::{ContextKey, Record};
use crate::body::{Unparsed, Unread};
use crate::condition::BodyUnread;
use crate::redaction::REDACTED;
use crate::request_line::{HostReading, ReportedMethod, RequestLine};

// What a value that can't be told is written as.
const AMBIGUOUS: &str = "[ambiguous]";

// The host of the catch-all's record past the group ceiling, and of the calls to hosts first seen
// past it.
const OVERFLOW: &str = "[overflow]";

/// Why a call an entry could have taken went to the catch-all with its body unread, each counted
/// apart and written as their total.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
pub struct Window<'p> {
    plan: &'p CompiledPlan,
    start: SystemTime,
    /// In the order opened, each a record of the document.
    records: Vec<OpenRecord>,
    /// Each record's index, by what tells it apart.
    groups: HashMap<GroupKey<'p>, usize>,
    overflow: Option<usize>,
    /// In the order first seen.
    hosts: Vec<HostTotal>,
    host_index: HashMap<String, usize>,
    /// When each call was received, and the record counting it.
    steps: Vec<(SystemTime, usize)>,
    steps_dropped: u64,
    body_unread: HashMap<BodyUnreadReason, u64>,
    calls_coarsened: u64,
    calls_dropped: u64,
    path_unresolved: u64,
    values_replaced: u64,
    values_ambiguous: u64,
}

// A record's entry and its context keys' values.
#[derive(Debug, PartialEq, Eq, Hash)]
struct GroupKey<'p> {
    entry: Option<&'p str>,
    values: Vec<Option<String>>,
}

#[derive(Debug)]
struct OpenRecord {
    entry_id: String,
    context_keys: Vec<(String, Option<String>)>,
    field_types: Vec<(String, String)>,
    measures: Vec<(String, u64)>,
}

// What a record writes of one call, and what it lost.
struct Written {
    values: Vec<Option<String>>,
    measures: Vec<u64>,
    path_unresolved: bool,
    values_replaced: u64,
    ambiguous: bool,
}

impl<'p> Window<'p> {
    pub fn open(plan: &'p CompiledPlan, start: SystemTime) -> Self {
        Self {
            plan,
            start,
            records: Vec::new(),
            groups: HashMap::new(),
            overflow: None,
            hosts: Vec::new(),
            host_index: HashMap::new(),
            steps: Vec::new(),
            steps_dropped: 0,
            body_unread: HashMap::new(),
            calls_coarsened: 0,
            calls_dropped: 0,
            path_unresolved: 0,
            values_replaced: 0,
            values_ambiguous: 0,
        }
    }

    /// Counts `call`, received at `at`; why its body went unread, if an entry could have taken it.
    pub fn record(&mut self, call: &ObservedCall, at: SystemTime) -> Option<BodyUnreadReason> {
        let CapturedCall {
            request_line,
            taken,
            record,
        } = self.plan.capture(call);
        let reason = body_unread_reason(&taken);
        if let Some(reason) = reason {
            *self.body_unread.entry(reason).or_default() += 1;
        }
        let at_ceiling = self.records.len() >= self.max_groups();
        self.count_host(request_line.host(), at_ceiling);
        let (entry, spec) = match &taken {
            Taken::Entry(entry) => (Some(entry.id.as_str()), &entry.record),
            Taken::CatchAll(catch_all, _) => (None, &catch_all.record),
        };
        let written = write(spec, &record, &request_line);
        let key = GroupKey {
            entry,
            values: written.values.clone(),
        };
        let index = match self.groups.get(&key) {
            Some(&index) => self.add(index, written),
            None if !at_ceiling => self.open_record(key, spec, written),
            // Past the ceiling: an entry's call in the catch-all, a catch-all's in its overflow.
            None => {
                self.calls_coarsened += 1;
                let catch_all = &self.plan.plan().catch_all;
                match entry {
                    Some(_) => {
                        let record = self.plan.read_catch_all(call, &request_line);
                        let written = write(&catch_all.record, &record, &request_line);
                        let key = GroupKey {
                            entry: None,
                            values: written.values.clone(),
                        };
                        match self.groups.get(&key) {
                            Some(&index) => self.add(index, written),
                            None => self.add_overflow(written),
                        }
                    }
                    None => self.add_overflow(written),
                }
            }
        };
        match self.plan.plan().budget.max_steps_per_window {
            Some(max) if self.steps.len() < max as usize => self.steps.push((at, index)),
            Some(_) => self.steps_dropped += 1,
            None => {}
        }
        reason
    }

    /// Counts calls lost before reaching the window.
    pub fn count_dropped(&mut self, calls: u64) {
        self.calls_dropped = self.calls_dropped.saturating_add(calls);
    }

    /// The window's document, for the sender to give its envelope.
    pub fn close(self, end: SystemTime) -> ClosedWindow {
        let plan = self.plan.plan();
        let record_id = |index: usize| format!("r{}", index + 1);
        let sequence = plan.budget.max_steps_per_window.map(|_| {
            self.steps
                .iter()
                .map(|&(at, index)| Step {
                    at: Timestamp(at),
                    record_id: record_id(index),
                })
                .collect()
        });
        let records = self
            .records
            .into_iter()
            .enumerate()
            .map(|(index, record)| CallRecord {
                record_id: record_id(index),
                entry_id: record.entry_id,
                context_keys: Fields(record.context_keys),
                field_types: Fields(record.field_types),
                measures: Fields(record.measures),
            })
            .collect();
        let truncated = Truncated {
            calls_coarsened: self.calls_coarsened,
            calls_dropped: self.calls_dropped,
            entries_skipped: self.plan.entries_skipped() as u64,
            body_unread: self.body_unread.values().sum(),
            windows_dropped: 0,
            path_unresolved: self.path_unresolved,
            values_replaced: self.values_replaced,
            values_ambiguous: self.values_ambiguous,
            steps_dropped: sequence.as_ref().map(|_| self.steps_dropped),
        };
        ClosedWindow(Captured {
            agent: plan.agent.clone(),
            plan_version: plan.plan_version,
            plan_hash: plan.content_hash.clone(),
            start: self.start,
            end,
            truncated,
            hosts: self.hosts,
            records,
            sequence,
        })
    }

    fn max_groups(&self) -> usize {
        self.plan.plan().budget.max_groups_per_window as usize
    }

    // Past the ceiling, a host not yet listed is totalled under the overflow marker.
    fn count_host(&mut self, host: &str, at_ceiling: bool) {
        let host = match self.host_index.contains_key(host) {
            false if at_ceiling => OVERFLOW,
            _ => host,
        };
        let index = match self.host_index.get(host) {
            Some(&index) => index,
            None => {
                self.hosts.push(HostTotal {
                    host: host.to_owned(),
                    count: 0,
                    error_count: 0,
                });
                self.host_index
                    .insert(host.to_owned(), self.hosts.len() - 1);
                self.hosts.len() - 1
            }
        };
        self.hosts[index].count += 1;
    }

    fn open_record(&mut self, key: GroupKey<'p>, spec: &Record, written: Written) -> usize {
        let plan = self.plan.plan();
        let entry_id = key.entry.unwrap_or(&plan.catch_all.id).to_owned();
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
        let measures = spec
            .measures
            .iter()
            .map(|measure| (measure.name().to_owned(), 0))
            .collect();
        self.records.push(OpenRecord {
            entry_id,
            context_keys,
            field_types,
            measures,
        });
        let index = self.records.len() - 1;
        self.groups.insert(key, index);
        self.add(index, written)
    }

    // The catch-all's one record past the ceiling, keeping only the host, as the overflow marker.
    fn add_overflow(&mut self, written: Written) -> usize {
        let index = match self.overflow {
            Some(index) => index,
            None => {
                let catch_all = &self.plan.plan().catch_all;
                let measures = catch_all
                    .record
                    .measures
                    .iter()
                    .map(|measure| (measure.name().to_owned(), 0))
                    .collect();
                self.records.push(OpenRecord {
                    entry_id: catch_all.id.clone(),
                    context_keys: vec![("host".to_owned(), Some(OVERFLOW.to_owned()))],
                    field_types: Vec::new(),
                    measures,
                });
                self.overflow = Some(self.records.len() - 1);
                self.records.len() - 1
            }
        };
        // Its keys aren't written, so neither is what they lost.
        for (slot, bytes) in self.records[index]
            .measures
            .iter_mut()
            .zip(&written.measures)
        {
            slot.1 = slot.1.saturating_add(*bytes);
        }
        index
    }

    fn add(&mut self, index: usize, written: Written) -> usize {
        for (slot, bytes) in self.records[index]
            .measures
            .iter_mut()
            .zip(&written.measures)
        {
            slot.1 = slot.1.saturating_add(*bytes);
        }
        self.path_unresolved += u64::from(written.path_unresolved);
        self.values_replaced += written.values_replaced;
        self.values_ambiguous += u64::from(written.ambiguous);
        index
    }
}

/// A closed window, waiting for its envelope.
#[derive(Debug)]
pub struct ClosedWindow(Captured);

impl ClosedWindow {
    pub fn into_document(self, envelope: Envelope) -> Document {
        Document::capturing(self.0, envelope)
    }
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

// A named key's or a measure's path finding nothing counts the call as unresolved; a built-in
// key has no path to find.
fn write(spec: &Record, record: &RecordReading, request_line: &RequestLine) -> Written {
    let mut written = Written {
        values: Vec::with_capacity(record.context_keys.len()),
        measures: Vec::with_capacity(record.measures.len()),
        path_unresolved: false,
        values_replaced: 0,
        ambiguous: false,
    };
    debug_assert_eq!(spec.context_keys.len(), record.context_keys.len());
    for reading in &record.context_keys {
        let value = match reading {
            KeyReading::Method(method) => {
                if *method == ReportedMethod::Other {
                    written.values_replaced += 1;
                }
                Some(method.as_str().to_owned())
            }
            KeyReading::Host(host) => {
                if *host == HostReading::PastLongest {
                    written.values_replaced += 1;
                }
                Some(request_line.host().to_owned())
            }
            KeyReading::PathPrefix(prefix) => Some(prefix.clone()),
            KeyReading::Operation(reading) => write_value(reading, &mut written, false),
            KeyReading::Path(reading) => write_value(reading, &mut written, true),
        };
        written.values.push(value);
    }
    for reading in &record.measures {
        let bytes = match reading {
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
        };
        written.measures.push(bytes);
    }
    written
}

// A value found, kept or replaced; `null` for nothing found, which a plan path counts as unresolved.
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
