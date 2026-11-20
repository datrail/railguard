use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde_json::Value;

use super::environment::{CaptureRequest, Environment};
use super::grammar::{
    BodyPath, BodyStep, Condition, Path, Predicate, RequestPath, Scalar, TicketPath,
};
use crate::body::mcp::{McpReading, Unreadable};
use crate::body::tree::{self, Kind, Member, Step};
use crate::body::{BodyRead, Unparsed, Unread};
use crate::endpoint::Endpoint;

/// The steps one judgement may take, unless the caller sets its own.
pub const DEFAULT_STEP_LIMIT: u64 = 100_000;

/// Counts the work of one judgement, so a hostile input stops at the same step on every host.
#[derive(Debug)]
pub struct Budget {
    limit: u64,
    steps: u64,
    stop: Option<Arc<AtomicBool>>,
}

impl Budget {
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            steps: 0,
            stop: None,
        }
    }

    /// Spent at once when `stop` is set: a caller that no longer wants the judgement ends it.
    pub fn stopped_by(self, stop: Arc<AtomicBool>) -> Self {
        Self {
            stop: Some(stop),
            ..self
        }
    }

    fn charge(&mut self, steps: u64) -> Result<(), Refusal> {
        self.steps = self.steps.saturating_add(steps);
        let stopped = || {
            self.stop
                .as_ref()
                .is_some_and(|stop| stop.load(Ordering::Relaxed))
        };
        if self.steps > self.limit || stopped() {
            return Err(Refusal::BudgetExceeded);
        }
        Ok(())
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::new(DEFAULT_STEP_LIMIT)
    }
}

/// Why a judgement couldn't be reached. Never a deny, and never a condition that didn't hold.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// An `Unknown` or `Invalid` condition was reached.
    Uninterpretable,
    BudgetExceeded,
}

/// A condition in the form it is evaluated in.
#[derive(Debug)]
pub struct CompiledCondition(Node);

#[derive(Debug)]
enum Node {
    Refuse,
    Test(Path, Test),
}

#[derive(Debug)]
enum Test {
    Existing,
    NotExisting,
    Eq(Scalar),
    Ne(Scalar),
    Lt(f64),
    Lte(f64),
    Gt(f64),
    Gte(f64),
    /// Only the operand's strings and numbers: anything else is a member of nothing.
    In(Vec<Scalar>),
    Contains(Scalar),
    NotContains(Scalar),
    Matches(Glob),
}

/// What a path resolves to, borrowed from the environment.
#[derive(Clone, Debug)]
pub enum Lookup<'a> {
    /// Also a `null`, which a producer may write for a missing claim.
    Absent,
    /// Decoded where the body writes it with an escape.
    String(Cow<'a, str>),
    /// A body's number past a double saturated to ±inf.
    Number(f64),
    /// Never compared: `true == 1` in some languages and not in others.
    Bool(bool),
    /// A header with several values, or a list in the ticket or the body, which may hold anything.
    List(Items<'a>),
    Object,
    /// A body value that can't be told: readers differ on it.
    Ambiguous,
    BodyUnread(BodyUnread),
}

/// A list's items, from where it's read.
#[derive(Clone, Copy, Debug)]
pub enum Items<'a> {
    Values(&'a [Value]),
    Body(tree::Node<'a>),
}

impl<'a> Items<'a> {
    pub fn iter(self) -> Box<dyn Iterator<Item = Lookup<'a>> + 'a> {
        match self {
            Self::Values(values) => Box::new(values.iter().map(read_value)),
            Self::Body(node) => Box::new(node.items().map(read_node)),
        }
    }
}

/// Why a body value isn't read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyUnread {
    /// The body isn't: not kept, not sent, or cut short.
    Unread(Unread),
    /// The body is, but not as a tree: not JSON, or its tree past the budget.
    Unparsed(Unparsed),
    /// A JSON-RPC batch, for what names one MCP message.
    Batch,
}

pub fn compile_condition(condition: &Condition) -> CompiledCondition {
    let Condition::Valid(predicate) = condition else {
        return CompiledCondition(Node::Refuse);
    };
    let (path, test) = match predicate {
        Predicate::Existing { path } => (path, Test::Existing),
        Predicate::NotExisting { path } => (path, Test::NotExisting),
        Predicate::Eq { path, value } => (path, Test::Eq(value.clone())),
        Predicate::Ne { path, value } => (path, Test::Ne(value.clone())),
        Predicate::Lt { path, value } => (path, Test::Lt(*value)),
        Predicate::Lte { path, value } => (path, Test::Lte(*value)),
        Predicate::Gt { path, value } => (path, Test::Gt(*value)),
        Predicate::Gte { path, value } => (path, Test::Gte(*value)),
        Predicate::In { path, value } => (path, Test::In(read_scalars(value))),
        Predicate::Contains { path, value } => (path, Test::Contains(value.clone())),
        Predicate::NotContains { path, value } => (path, Test::NotContains(value.clone())),
        Predicate::Matches { path, value } => (path, Test::Matches(Glob::new(value))),
    };
    CompiledCondition(Node::Test(path.clone(), test))
}

/// What evaluating a condition found.
#[derive(Debug)]
pub struct Evaluation<'a> {
    /// What the condition's path resolved to, or `None` if none was: the condition can't be read,
    /// or the budget ran out first.
    pub resolved: Option<Lookup<'a>>,
    pub verdict: Verdict,
}

/// Whether a condition held. The others are neither, each for its own reason.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Holds,
    DoesNotHold,
    /// The value read can't be told.
    Ambiguous,
    BodyUnread(BodyUnread),
    Refused(Refusal),
}

impl From<Result<bool, Refusal>> for Verdict {
    fn from(result: Result<bool, Refusal>) -> Self {
        match result {
            Ok(true) => Self::Holds,
            Ok(false) => Self::DoesNotHold,
            Err(refusal) => Self::Refused(refusal),
        }
    }
}

pub fn evaluate_condition<'a>(
    condition: &CompiledCondition,
    environment: &Environment<'a>,
    budget: &mut Budget,
) -> Evaluation<'a> {
    let refuse = |refusal| Evaluation {
        resolved: None,
        verdict: Verdict::Refused(refusal),
    };
    if let Err(refusal) = budget.charge(1) {
        return refuse(refusal);
    }
    let Node::Test(path, test) = &condition.0 else {
        return refuse(Refusal::Uninterpretable);
    };
    let value = match resolve(path, environment, budget) {
        Ok(value) => value,
        Err(refusal) => return refuse(refusal),
    };
    let verdict = match &value {
        Lookup::Ambiguous => Verdict::Ambiguous,
        Lookup::BodyUnread(why) => Verdict::BodyUnread(*why),
        _ => apply(test, &value, budget),
    };
    Evaluation {
        resolved: Some(value),
        verdict,
    }
}

fn read_scalars(items: &[Value]) -> Vec<Scalar> {
    items
        .iter()
        .filter_map(|item| match item {
            Value::String(text) => Some(Scalar::String(text.clone())),
            Value::Number(number) => number.as_f64().map(Scalar::Number),
            _ => None,
        })
        .collect()
}

fn resolve<'a>(
    path: &Path,
    environment: &Environment<'a>,
    budget: &mut Budget,
) -> Result<Lookup<'a>, Refusal> {
    Ok(match path {
        Path::Ticket(path) => resolve_ticket(path, environment),
        Path::Request(path) => resolve_request(path, environment),
        Path::Body(path) => return resolve_body(path, environment, budget),
    })
}

/// One key at a time, so a step that is missing, `null` or not an object resolves to absent.
fn resolve_ticket<'a>(path: &TicketPath, environment: &Environment<'a>) -> Lookup<'a> {
    let Some(claims) = environment.ticket else {
        return Lookup::Absent;
    };
    let Some((first, rest)) = path.0.split_first() else {
        return Lookup::Object;
    };
    let mut value = claims.get(first);
    for key in rest {
        value = value
            .and_then(Value::as_object)
            .and_then(|object| object.get(key));
    }
    value.map_or(Lookup::Absent, read_value)
}

fn read_value(value: &Value) -> Lookup<'_> {
    match value {
        Value::Null => Lookup::Absent,
        Value::Bool(value) => Lookup::Bool(*value),
        // Always some double, since serde_json reads no number past one.
        Value::Number(number) => number.as_f64().map_or(Lookup::Absent, Lookup::Number),
        Value::String(text) => Lookup::String(Cow::Borrowed(text)),
        Value::Array(values) => Lookup::List(Items::Values(values)),
        Value::Object(_) => Lookup::Object,
    }
}

fn resolve_request<'a>(path: &RequestPath, environment: &Environment<'a>) -> Lookup<'a> {
    let field = |read: fn(&CaptureRequest) -> &str| {
        environment.request.map_or(Lookup::Absent, |request| {
            Lookup::String(Cow::Borrowed(read(request)))
        })
    };
    match path {
        RequestPath::Whole | RequestPath::Headers => Lookup::Object,
        RequestPath::EndpointKey => match environment.endpoint {
            Endpoint::Resolved(key) => Lookup::String(Cow::Borrowed(key.as_str())),
            Endpoint::Keyless | Endpoint::Unrecognised => Lookup::Absent,
        },
        RequestPath::Header(name) => match environment.headers.get(name) {
            None => Lookup::Absent,
            Some([value]) => read_value(value),
            Some(values) => Lookup::List(Items::Values(values)),
        },
        RequestPath::Host => field(|request| &request.host),
        RequestPath::Method => field(|request| &request.method),
        RequestPath::Path => field(|request| &request.path),
        RequestPath::RedactedPath => field(|request| &request.redacted_path),
        RequestPath::McpMethod => read_mcp(environment, McpReading::method),
        RequestPath::McpTool => read_mcp(environment, McpReading::tool),
    }
}

fn read_mcp<'a>(
    environment: &Environment<'a>,
    read: fn(&McpReading) -> Option<&str>,
) -> Lookup<'a> {
    let body = match environment.body {
        None => return Lookup::Absent,
        Some(BodyRead::Unread(why)) => return Lookup::BodyUnread(BodyUnread::Unread(*why)),
        Some(BodyRead::Whole(body)) => body,
    };
    match body.mcp() {
        McpReading::Unreadable(Unreadable::Unparsed(why)) => {
            Lookup::BodyUnread(BodyUnread::Unparsed(*why))
        }
        McpReading::Unreadable(Unreadable::Batch) => Lookup::BodyUnread(BodyUnread::Batch),
        McpReading::Unreadable(Unreadable::Ambiguous) => Lookup::Ambiguous,
        reading @ McpReading::Read { .. } => {
            read(reading).map_or(Lookup::Absent, |text| Lookup::String(Cow::Borrowed(text)))
        }
    }
}

/// An index's hops charged, one per item passed.
fn resolve_body<'a>(
    path: &BodyPath,
    environment: &Environment<'a>,
    budget: &mut Budget,
) -> Result<Lookup<'a>, Refusal> {
    let body = match environment.body {
        None => return Ok(Lookup::Absent),
        Some(BodyRead::Unread(why)) => return Ok(Lookup::BodyUnread(BodyUnread::Unread(*why))),
        Some(BodyRead::Whole(body)) => body,
    };
    // An empty body, as an event stream's `GET` sends, holds nothing.
    if body.as_bytes().is_empty() {
        return Ok(Lookup::Absent);
    }
    let root = match body.json() {
        Ok(root) => root,
        Err(why) => return Ok(Lookup::BodyUnread(BodyUnread::Unparsed(why))),
    };
    let mut steps = Vec::with_capacity(path.0.len());
    for step in &path.0 {
        steps.push(match step {
            BodyStep::Key(key) => Step::Key(key),
            BodyStep::Index(index) => {
                budget.charge(*index as u64 + 1)?;
                Step::Index(*index)
            }
        });
    }
    Ok(match root.walk(&steps) {
        Member::Found(node) => read_node(node),
        Member::Absent => Lookup::Absent,
        Member::Ambiguous => Lookup::Ambiguous,
    })
}

fn read_node(node: tree::Node<'_>) -> Lookup<'_> {
    match node.kind() {
        Kind::Null => Lookup::Absent,
        Kind::Bool(value) => Lookup::Bool(value),
        Kind::Number => std::str::from_utf8(node.raw())
            .ok()
            .and_then(|text| text.parse().ok())
            .map_or(Lookup::Ambiguous, Lookup::Number),
        // One that doesn't decode: readers differ on what it holds.
        Kind::String => node.text().map_or(Lookup::Ambiguous, Lookup::String),
        Kind::Array => Lookup::List(Items::Body(node)),
        Kind::Object => Lookup::Object,
    }
}

// A list's items read once, so the tests see them as values; one that can't be told makes the
// list so.
fn apply(test: &Test, value: &Lookup, budget: &mut Budget) -> Verdict {
    let mut items = None;
    if let Lookup::List(list) = value {
        let read: Vec<Lookup> = list.iter().collect();
        if let Err(refusal) = budget.charge(read.len() as u64) {
            return Verdict::Refused(refusal);
        }
        if read.iter().any(|item| matches!(item, Lookup::Ambiguous)) {
            return Verdict::Ambiguous;
        }
        items = Some(read);
    }
    holds(test, value, items.as_deref(), budget).into()
}

fn holds(
    test: &Test,
    value: &Lookup,
    items: Option<&[Lookup]>,
    budget: &mut Budget,
) -> Result<bool, Refusal> {
    Ok(match (test, value, items) {
        (Test::Existing, value, _) => !matches!(value, Lookup::Absent),
        (Test::NotExisting, value, _) => matches!(value, Lookup::Absent),
        // On a list, `eq` needs every value to equal, and `ne` any value to differ.
        (Test::Eq(operand), _, Some(items)) => {
            !items.is_empty() && items.iter().all(|item| equals(item, operand))
        }
        (Test::Eq(operand), value, None) => equals(value, operand),
        (Test::Ne(operand), _, Some(items)) => {
            !items.is_empty() && items.iter().any(|item| !equals(item, operand))
        }
        (Test::Ne(operand), value, None) => is_scalar(value) && !equals(value, operand),
        (Test::Lt(operand), Lookup::Number(number), _) => number < operand,
        (Test::Lte(operand), Lookup::Number(number), _) => number <= operand,
        (Test::Gt(operand), Lookup::Number(number), _) => number > operand,
        (Test::Gte(operand), Lookup::Number(number), _) => number >= operand,
        (Test::In(operand), _, Some(items)) => {
            budget.charge((items.len() as u64).saturating_mul(operand.len() as u64))?;
            items.iter().any(|item| is_member(item, operand))
        }
        (Test::In(operand), value, None) => {
            budget.charge(operand.len() as u64)?;
            is_member(value, operand)
        }
        (Test::Contains(operand), _, Some(items)) => items.iter().any(|item| equals(item, operand)),
        (Test::NotContains(operand), _, Some(items)) => {
            !items.iter().any(|item| equals(item, operand))
        }
        (Test::Matches(glob), _, Some(items)) => {
            for item in items {
                if let Lookup::String(text) = item
                    && glob.matches(text, budget)?
                {
                    return Ok(true);
                }
            }
            false
        }
        (Test::Matches(glob), Lookup::String(text), None) => glob.matches(text, budget)?,
        _ => false,
    })
}

/// A string or a number: the only kinds that compare, and only with their own kind.
fn is_scalar(value: &Lookup) -> bool {
    matches!(value, Lookup::String(_) | Lookup::Number(_))
}

fn equals(value: &Lookup, operand: &Scalar) -> bool {
    match (value, operand) {
        (Lookup::Number(number), Scalar::Number(operand)) => number == operand,
        (Lookup::String(text), Scalar::String(operand)) => text == operand,
        _ => false,
    }
}

fn is_member(value: &Lookup, operand: &[Scalar]) -> bool {
    operand.iter().any(|item| equals(value, item))
}

/// A whole-string pattern where `*` is the only metacharacter.
#[derive(Debug)]
struct Glob {
    /// The literals between stars; one segment means no star.
    segments: Vec<String>,
    /// Charged with the value, since each literal searched costs its length too.
    pattern_len: usize,
}

impl Glob {
    fn new(pattern: &str) -> Self {
        Self {
            segments: pattern.split('*').map(str::to_string).collect(),
            pattern_len: pattern.len(),
        }
    }

    /// One left-to-right scan, never backtracking: each literal is matched at its earliest place.
    fn matches(&self, value: &str, budget: &mut Budget) -> Result<bool, Refusal> {
        budget.charge(1 + (value.len() + self.pattern_len) as u64 / 64)?;
        let [first, middle @ .., last] = self.segments.as_slice() else {
            return Ok(self.segments[0] == value);
        };
        if !value.starts_with(first.as_str()) || !value.ends_with(last.as_str()) {
            return Ok(false);
        }
        // The two ends can't share characters: `ab*ab` doesn't match `ab`.
        if first.len() + last.len() > value.len() {
            return Ok(false);
        }
        let mut rest = &value[first.len()..value.len() - last.len()];
        for literal in middle {
            match rest.find(literal.as_str()) {
                Some(found) => rest = &rest[found + literal.len()..],
                None => return Ok(false),
            }
        }
        Ok(true)
    }
}
