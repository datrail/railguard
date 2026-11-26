//! A call matched against a compiled plan, and what the record counting it, an entry's or the
//! catch-all's, reads of it.

use sha2::{Digest, Sha256};

use super::compile::{CompiledEntry, CompiledPlan, EntryRefusal};
use super::plan::{CatchAll, ContextKey, Entry, KeyFunction, Measure, MeasureFunction, Record};
use crate::condition::{
    BodyPath, BodyUnread, Budget, Environment, Items, Lookup, Path, Refusal, RequestPath, Verdict,
    evaluate_condition, resolve_path,
};
use crate::redaction::{REDACTED, is_secret_token};
use crate::request::ObservedRequest;
use crate::request_line::{HostReading, ReportedMethod, Unmatchable};

// Past this, a kept value could carry data of any length.
const MAX_KEPT_CHARS: usize = 256;

/// A call matched against a plan, and what the record counting it reads of it.
#[derive(Debug)]
pub struct CapturedCall<'p> {
    pub taken: Taken<'p>,
    /// What the record counting it reads: the taking entry's, or the catch-all's.
    pub record: RecordReading,
}

/// Which of the plan's records counts a call.
#[derive(Debug)]
pub enum Taken<'p> {
    Entry(&'p Entry),
    CatchAll(&'p CatchAll, Untaken<'p>),
}

impl Taken<'_> {
    /// The plan's record counting the call, and its id.
    pub(crate) fn record(&self) -> (&str, &Record) {
        match self {
            Self::Entry(entry) => (&entry.id, &entry.record),
            Self::CatchAll(catch_all, _) => (&catch_all.id, &catch_all.record),
        }
    }
}

/// Why the catch-all counts a call: the entry named is where its walk stopped.
#[derive(Debug)]
pub enum Untaken<'p> {
    /// No entry's conditions all hold.
    NoEntry,
    /// Its path matches no entry, whatever their conditions.
    Unmatchable(Unmatchable),
    /// An entry with a condition that can't be evaluated.
    Refused(&'p Entry, &'p EntryRefusal),
    /// An entry whose conditions were refused as they were evaluated: the step budget spent.
    Stopped(&'p Entry, Refusal),
    /// An entry's conditions read a body value that can't be told.
    Ambiguous(&'p Entry),
    /// An entry's conditions read a body that isn't.
    BodyUnread(&'p Entry, BodyUnread),
}

/// What a record reads of a call, in the plan's order.
#[derive(Debug, PartialEq)]
pub struct RecordReading {
    /// One per context key.
    pub context_keys: Vec<KeyReading>,
    /// One per measure.
    pub measures: Vec<MeasureReading>,
}

#[derive(Debug, PartialEq)]
pub enum KeyReading {
    Method(ReportedMethod),
    Host(HostReading),
    /// The redacted path's first segments.
    PathPrefix(String),
    /// The tool a `tools/call` names, read as a path to it is.
    Operation(PathReading<KeptValue>),
    /// A named key's path.
    Path(PathReading<KeptValue>),
}

#[derive(Debug, PartialEq)]
pub enum MeasureReading {
    /// What each of its paths adds.
    SizeInBytes(Vec<PathReading<u64>>),
    /// Whether each of its paths finds a value; nothing there is found, as not set.
    IsSet(Vec<PathReading<bool>>),
}

/// What a record's path found.
#[derive(Debug, PartialEq)]
pub enum PathReading<T> {
    Found(T),
    /// Nothing there, or a JSON `null`.
    Absent,
    /// A value that can't be told: readers differ on it, or, for a context key, a query parameter
    /// sent several times.
    Ambiguous,
    BodyUnread(BodyUnread),
    /// The step budget spent.
    Refused(Refusal),
}

/// A context key's value found.
#[derive(Debug, PartialEq)]
pub enum KeptValue {
    String(String),
    /// A string too, its SHA-256 kept, as 64 lower-case hex digits.
    Hashed(String),
    /// A string too, not kept.
    Replaced(Replacement),
    /// Anything else, which a context key doesn't keep.
    NotAString,
}

/// Why a string found isn't kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replacement {
    /// The whole of it in a shape some vendor issues credentials in.
    KeyShape,
    /// Past 256 characters.
    PastLongest,
}

impl CompiledPlan {
    /// Matches `call`, then reads what the record counting it holds.
    pub fn capture(&self, call: &ObservedRequest) -> CapturedCall<'_> {
        let mut reader = CallReader::new(call);
        let taken = reader.take(self);
        let record = reader.read(taken.record().1);
        CapturedCall { taken, record }
    }

    // The first entry taking the call, unless the walk stops first.
    fn walk(&self, call: &ObservedRequest, budget: &mut Budget) -> Result<&Entry, Untaken<'_>> {
        let request_line = call.request_line();
        if let Some(why) = request_line.unmatchable() {
            return Err(Untaken::Unmatchable(why));
        }
        let environment = capture_environment(call);
        for (entry, compiled) in self.entries_for(request_line.host()) {
            if let Some(refused) = compiled.refusal() {
                return Err(Untaken::Refused(entry, refused));
            }
            match answer(compiled, &environment, budget) {
                Verdict::Holds => return Ok(entry),
                Verdict::DoesNotHold => {}
                Verdict::Ambiguous => return Err(Untaken::Ambiguous(entry)),
                Verdict::BodyUnread(why) => return Err(Untaken::BodyUnread(entry, why)),
                Verdict::Refused(refusal) => return Err(Untaken::Stopped(entry, refusal)),
            }
        }
        Err(Untaken::NoEntry)
    }
}

/// One call as it's read: the call, and its step budget, spent by matching it and reading its
/// record alike.
pub(crate) struct CallReader<'c> {
    call: &'c ObservedRequest,
    budget: Budget,
}

impl<'c> CallReader<'c> {
    pub(crate) fn new(call: &'c ObservedRequest) -> Self {
        Self {
            call,
            budget: Budget::default(),
        }
    }

    /// Which of `plan`'s records counts the call.
    pub(crate) fn take<'p>(&mut self, plan: &'p CompiledPlan) -> Taken<'p> {
        match plan.walk(self.call, &mut self.budget) {
            Ok(entry) => Taken::Entry(entry),
            Err(why) => Taken::CatchAll(&plan.plan().catch_all, why),
        }
    }

    /// What `record` reads of the call.
    pub(crate) fn read(&mut self, record: &Record) -> RecordReading {
        read_record(record, self.call, &mut self.budget)
    }
}

// The proxy reads no ticket: it strips the agent's.
pub(crate) fn capture_environment(call: &ObservedRequest) -> Environment<'_> {
    Environment::of(call, None)
}

// The entry's "and": a condition that doesn't hold, or a refusal, decides it; else the first that
// can't be told.
pub(crate) fn answer(
    entry: &CompiledEntry,
    environment: &Environment,
    budget: &mut Budget,
) -> Verdict {
    let mut answer = Verdict::Holds;
    for condition in entry.conditions() {
        match evaluate_condition(condition, environment, budget).verdict {
            Verdict::Holds => {}
            decided @ (Verdict::DoesNotHold | Verdict::Refused(_)) => return decided,
            untold @ (Verdict::Ambiguous | Verdict::BodyUnread(_)) => {
                if answer == Verdict::Holds {
                    answer = untold;
                }
            }
        }
    }
    answer
}

pub(crate) fn read_record(
    record: &Record,
    call: &ObservedRequest,
    budget: &mut Budget,
) -> RecordReading {
    let request_line = call.request_line();
    let environment = &capture_environment(call);
    let body_bytes = call.body_bytes;
    let context_keys = record
        .context_keys
        .iter()
        .map(|key| match key {
            ContextKey::Method => KeyReading::Method(request_line.reported_method()),
            ContextKey::Host => KeyReading::Host(request_line.host_reading().clone()),
            ContextKey::PathPrefix { segments } => {
                KeyReading::PathPrefix(request_line.path_prefix(*segments as usize).to_owned())
            }
            ContextKey::Operation => {
                let tool = Path::Request(RequestPath::McpTool);
                KeyReading::Operation(read_key(&tool, KeyFunction::None, environment, budget))
            }
            ContextKey::Named { path, function, .. } => {
                KeyReading::Path(read_key(path, *function, environment, budget))
            }
        })
        .collect();
    let measures = record
        .measures
        .iter()
        .map(
            |Measure {
                 function, paths, ..
             }| match function {
                MeasureFunction::SizeInBytes => MeasureReading::SizeInBytes(
                    paths
                        .iter()
                        .map(|path| read_size_in_bytes(path, environment, body_bytes, budget))
                        .collect(),
                ),
                MeasureFunction::IsSet => MeasureReading::IsSet(
                    paths
                        .iter()
                        .map(|path| read_is_set(path, environment, budget))
                        .collect(),
                ),
            },
        )
        .collect();
    RecordReading {
        context_keys,
        measures,
    }
}

fn read_key(
    path: &Path,
    function: KeyFunction,
    environment: &Environment,
    budget: &mut Budget,
) -> PathReading<KeptValue> {
    let lookup = match resolve_path(path, environment, budget) {
        Ok((lookup, _)) => lookup,
        Err(refusal) => return PathReading::Refused(refusal),
    };
    match lookup {
        Lookup::String(text) => PathReading::Found(match function {
            KeyFunction::None => keep(&text),
            // The redaction's marker, kept so the loss still reads as one.
            KeyFunction::Sha256
                if text == REDACTED
                    && matches!(path, Path::Request(RequestPath::RedactedPath(_))) =>
            {
                KeptValue::String(text.into_owned())
            }
            // Its bounds are on what leaves, and a hash leaves none of the string.
            KeyFunction::Sha256 => KeptValue::Hashed(hash(&text)),
        }),
        // A parameter sent several times: which one a server reads differs.
        Lookup::List(_)
            if matches!(path, Path::Request(RequestPath::Query { index: None, .. })) =>
        {
            PathReading::Ambiguous
        }
        Lookup::Number(_) | Lookup::Bool(_) | Lookup::List(_) | Lookup::Object => {
            PathReading::Found(KeptValue::NotAString)
        }
        Lookup::Absent => PathReading::Absent,
        Lookup::Ambiguous => PathReading::Ambiguous,
        Lookup::BodyUnread(why) => PathReading::BodyUnread(why),
    }
}

// Any value but a JSON `null`: a parameter sent several times is set, whichever a server reads.
fn read_is_set(path: &Path, environment: &Environment, budget: &mut Budget) -> PathReading<bool> {
    match resolve_path(path, environment, budget) {
        Ok((Lookup::Absent, _)) => PathReading::Found(false),
        // In the body, whether or not it decodes.
        Ok((_, Some(_))) => PathReading::Found(true),
        Ok((Lookup::Ambiguous, None)) => PathReading::Ambiguous,
        Ok((Lookup::BodyUnread(why), _)) => PathReading::BodyUnread(why),
        Ok(_) => PathReading::Found(true),
        Err(refusal) => PathReading::Refused(refusal),
    }
}

fn keep(text: &str) -> KeptValue {
    if is_secret_token(text) {
        KeptValue::Replaced(Replacement::KeyShape)
    } else if text.chars().nth(MAX_KEPT_CHARS).is_some() {
        KeptValue::Replaced(Replacement::PastLongest)
    } else {
        KeptValue::String(text.to_owned())
    }
}

fn hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// A body value as the body writes it, a string's quotes and escapes included; a query value or a
// segment as the request line holds it.
fn read_size_in_bytes(
    path: &Path,
    environment: &Environment,
    body_bytes: u64,
    budget: &mut Budget,
) -> PathReading<u64> {
    // The whole body, as sent, which needs no parse.
    if let Path::Request(RequestPath::Body(BodyPath(steps))) = path
        && steps.is_empty()
    {
        return PathReading::Found(body_bytes);
    }
    let (lookup, node) = match resolve_path(path, environment, budget) {
        Ok(found) => found,
        Err(refusal) => return PathReading::Refused(refusal),
    };
    let bytes = match (lookup, node) {
        (Lookup::Absent, _) => return PathReading::Absent,
        (Lookup::BodyUnread(why), _) => return PathReading::BodyUnread(why),
        // Read from the body: its bytes whether or not they decode.
        (_, Some(node)) => node.raw().len(),
        (Lookup::Ambiguous, None) => return PathReading::Ambiguous,
        (Lookup::String(text), None) => text.len(),
        // A query parameter's values, each a string.
        (Lookup::List(Items::Values(values)), None) => values
            .iter()
            .map(|value| value.as_str().map_or(0, str::len))
            .sum(),
        // Only the body's values are other than strings; a record reads no other.
        (_, None) => 0,
    };
    PathReading::Found(bytes as u64)
}
