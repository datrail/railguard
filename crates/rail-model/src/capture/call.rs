//! A call as the callout hands it over, matched against a compiled plan, and what the entry that
//! takes it records of it.

use super::compile::{CompiledEntry, CompiledPlan, EntryRefusal};
use super::plan::{ContextKey, Entry, Function, Measure, Record};
use crate::body::BodyRead;
use crate::condition::{
    BodyPath, BodyUnread, Budget, Environment, Items, Lookup, Path, Refusal, RequestPath, Verdict,
    evaluate_condition, resolve_path,
};
use crate::redaction::is_secret_token;
use crate::request_line::{ReportedMethod, RequestLine, Unmatchable};

// Past this, a kept value could carry data of any length.
const MAX_KEPT_CHARS: usize = 256;

/// A call as the callout hands it to the capture thread, its request whole.
#[derive(Debug)]
pub struct ObservedCall {
    /// `:authority` as the agent sent it, before the proxy rewrites it.
    pub authority: Vec<u8>,
    pub method: Vec<u8>,
    /// `:path` as received: the path and its query, never decoded.
    pub path: Vec<u8>,
    pub body: BodyRead,
    /// Every byte of the body the callout was sent, kept or not.
    pub body_bytes: u64,
}

/// A call matched against a plan.
#[derive(Debug)]
pub struct CapturedCall<'p> {
    pub request_line: RequestLine,
    pub taken: Taken<'p>,
}

/// Which record counts a call.
#[derive(Debug)]
pub enum Taken<'p> {
    Entry {
        entry: &'p Entry,
        record: RecordReading,
    },
    CatchAll(Untaken<'p>),
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

/// What an entry's record reads of the call it takes, in the plan's order.
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
    Path(PathReading<KeptValue>),
}

#[derive(Debug, PartialEq)]
pub enum MeasureReading {
    CallCount,
    ErrorCount,
    /// The body's bytes as sent, read or not.
    RequestBytes(u64),
    /// What each of its paths adds.
    Size(Vec<PathReading<u64>>),
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
    /// Matches `call`, then reads what the entry that takes it records.
    pub fn capture(&self, call: &ObservedCall) -> CapturedCall<'_> {
        let request_line = RequestLine::read(&call.authority, &call.method, &call.path);
        let environment = Environment::for_capture(&request_line, &call.body);
        // One per call, matching and reading its record, as one judgement is.
        let mut budget = Budget::default();
        let taken = match self.walk(&request_line, &environment, &mut budget) {
            Ok(entry) => {
                let record = read_record(
                    &entry.record,
                    &request_line,
                    &environment,
                    call.body_bytes,
                    &mut budget,
                );
                Taken::Entry { entry, record }
            }
            Err(why) => Taken::CatchAll(why),
        };
        CapturedCall {
            request_line,
            taken,
        }
    }

    // The first entry taking the call, unless the walk stops first.
    fn walk(
        &self,
        request_line: &RequestLine,
        environment: &Environment,
        budget: &mut Budget,
    ) -> Result<&Entry, Untaken<'_>> {
        if let Some(why) = request_line.unmatchable() {
            return Err(Untaken::Unmatchable(why));
        }
        for (entry, compiled) in self.entries_for(request_line.host()) {
            if let Some(refused) = compiled.refusal() {
                return Err(Untaken::Refused(entry, refused));
            }
            match answer(compiled, environment, budget) {
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

// The entry's "and": a condition that doesn't hold, or a refusal, decides it; else the first that
// can't be told.
fn answer(entry: &CompiledEntry, environment: &Environment, budget: &mut Budget) -> Verdict {
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

fn read_record(
    record: &Record,
    request_line: &RequestLine,
    environment: &Environment,
    body_bytes: u64,
    budget: &mut Budget,
) -> RecordReading {
    let context_keys = record
        .context_keys
        .iter()
        .map(|key| match key {
            ContextKey::Method => KeyReading::Method(request_line.reported_method()),
            ContextKey::Named { path, .. } => KeyReading::Path(read_key(path, environment, budget)),
        })
        .collect();
    let measures = record
        .measures
        .iter()
        .map(|measure| match measure {
            Measure::CallCount => MeasureReading::CallCount,
            Measure::ErrorCount => MeasureReading::ErrorCount,
            Measure::RequestBytes => MeasureReading::RequestBytes(body_bytes),
            Measure::Named {
                function: Function::Size,
                paths,
                ..
            } => MeasureReading::Size(
                paths
                    .iter()
                    .map(|path| read_size(path, environment, body_bytes, budget))
                    .collect(),
            ),
        })
        .collect();
    RecordReading {
        context_keys,
        measures,
    }
}

fn read_key(path: &Path, environment: &Environment, budget: &mut Budget) -> PathReading<KeptValue> {
    let lookup = match resolve_path(path, environment, budget) {
        Ok((lookup, _)) => lookup,
        Err(refusal) => return PathReading::Refused(refusal),
    };
    match lookup {
        Lookup::String(text) => PathReading::Found(keep(&text)),
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

fn keep(text: &str) -> KeptValue {
    if is_secret_token(text) {
        KeptValue::Replaced(Replacement::KeyShape)
    } else if text.chars().nth(MAX_KEPT_CHARS).is_some() {
        KeptValue::Replaced(Replacement::PastLongest)
    } else {
        KeptValue::String(text.to_owned())
    }
}

// A string's UTF-8 length once escapes are undone, any other value's JSON without whitespace.
fn read_size(
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
    let bytes = match lookup {
        Lookup::String(text) => text.len(),
        // A query parameter's values, each a string.
        Lookup::List(Items::Values(values)) => values
            .iter()
            .map(|value| value.as_str().map_or(0, str::len))
            .sum(),
        // Only the body's values are read from a node; a record reads no other.
        Lookup::Number(_) | Lookup::Bool(_) | Lookup::List(Items::Body(_)) | Lookup::Object => {
            node.map_or(0, |node| json_without_whitespace(node.raw()))
        }
        Lookup::Absent => return PathReading::Absent,
        Lookup::Ambiguous => return PathReading::Ambiguous,
        Lookup::BodyUnread(why) => return PathReading::BodyUnread(why),
    };
    PathReading::Found(bytes as u64)
}

// The length of JSON text without the whitespace between its tokens.
fn json_without_whitespace(raw: &[u8]) -> usize {
    let (mut length, mut in_string, mut escaped) = (0, false, false);
    for &byte in raw {
        if in_string {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
        } else if matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
            continue;
        } else if byte == b'"' {
            in_string = true;
        }
        length += 1;
    }
    length
}
