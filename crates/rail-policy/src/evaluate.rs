use serde::Serialize;
use serde_json::Value;

use crate::condition::{Condition, Path, Predicate, RequestPath, Scalar, TicketPath};
use crate::endpoint::Endpoint;
use crate::environment::Environment;

/// The steps one judgement may take, unless the caller sets its own.
pub const DEFAULT_STEP_LIMIT: u64 = 100_000;

/// Counts the work of one judgement, so a hostile input stops at the same step on every host.
#[derive(Debug)]
pub struct Budget {
    limit: u64,
    steps: u64,
}

impl Budget {
    pub fn new(limit: u64) -> Self {
        Self { limit, steps: 0 }
    }

    fn charge(&mut self, steps: u64) -> Result<(), Refusal> {
        self.steps = self.steps.saturating_add(steps);
        if self.steps > self.limit {
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
#[derive(Clone, Copy, Debug)]
pub enum Lookup<'a> {
    /// Also a `null`, which a producer may write for a missing claim.
    Absent,
    String(&'a str),
    Number(f64),
    /// Never compared: `true == 1` in some languages and not in others.
    Bool(bool),
    /// A header with several values, or a list in the ticket, which may hold anything.
    List(&'a [Value]),
    Object,
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

/// Whether a condition held. A refusal is neither.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Holds,
    DoesNotHold,
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
    let value = resolve(path, environment);
    Evaluation {
        resolved: Some(value),
        verdict: apply(test, value, budget).into(),
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

fn resolve<'a>(path: &Path, environment: &Environment<'a>) -> Lookup<'a> {
    match path {
        Path::Ticket(path) => resolve_ticket(path, environment),
        Path::Request(path) => resolve_request(path, environment),
    }
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
        Value::String(text) => Lookup::String(text),
        Value::Array(values) => Lookup::List(values),
        Value::Object(_) => Lookup::Object,
    }
}

fn resolve_request<'a>(path: &RequestPath, environment: &Environment<'a>) -> Lookup<'a> {
    match path {
        RequestPath::Whole | RequestPath::Headers => Lookup::Object,
        RequestPath::EndpointKey => match environment.endpoint {
            Endpoint::Resolved(key) => Lookup::String(key.as_str()),
            Endpoint::Keyless | Endpoint::Unrecognised => Lookup::Absent,
        },
        RequestPath::Header(name) => match environment.headers.get(name) {
            None => Lookup::Absent,
            Some([value]) => read_value(value),
            Some(values) => Lookup::List(values),
        },
    }
}

fn apply(test: &Test, value: Lookup, budget: &mut Budget) -> Result<bool, Refusal> {
    if let Lookup::List(items) = value {
        budget.charge(items.len() as u64)?;
    }
    Ok(match (test, value) {
        (Test::Existing, value) => !matches!(value, Lookup::Absent),
        (Test::NotExisting, value) => matches!(value, Lookup::Absent),
        // On a list, `eq` needs every value to equal, and `ne` any value to differ.
        (Test::Eq(operand), Lookup::List(items)) => {
            !items.is_empty()
                && items
                    .iter()
                    .map(read_value)
                    .all(|item| equals(item, operand))
        }
        (Test::Eq(operand), value) => equals(value, operand),
        (Test::Ne(operand), Lookup::List(items)) => {
            !items.is_empty()
                && items
                    .iter()
                    .map(read_value)
                    .any(|item| !equals(item, operand))
        }
        (Test::Ne(operand), value) => is_scalar(value) && !equals(value, operand),
        (Test::Lt(operand), Lookup::Number(number)) => number < *operand,
        (Test::Lte(operand), Lookup::Number(number)) => number <= *operand,
        (Test::Gt(operand), Lookup::Number(number)) => number > *operand,
        (Test::Gte(operand), Lookup::Number(number)) => number >= *operand,
        (Test::In(operand), Lookup::List(items)) => {
            budget.charge((items.len() as u64).saturating_mul(operand.len() as u64))?;
            items
                .iter()
                .map(read_value)
                .any(|item| is_member(item, operand))
        }
        (Test::In(operand), value) => {
            budget.charge(operand.len() as u64)?;
            is_member(value, operand)
        }
        (Test::Contains(operand), Lookup::List(items)) => items
            .iter()
            .map(read_value)
            .any(|item| equals(item, operand)),
        (Test::NotContains(operand), Lookup::List(items)) => !items
            .iter()
            .map(read_value)
            .any(|item| equals(item, operand)),
        (Test::Matches(glob), Lookup::List(items)) => {
            for item in items.iter().map(read_value) {
                if let Lookup::String(text) = item
                    && glob.matches(text, budget)?
                {
                    return Ok(true);
                }
            }
            false
        }
        (Test::Matches(glob), Lookup::String(text)) => glob.matches(text, budget)?,
        _ => false,
    })
}

/// A string or a number: the only kinds that compare, and only with their own kind.
fn is_scalar(value: Lookup) -> bool {
    matches!(value, Lookup::String(_) | Lookup::Number(_))
}

fn equals(value: Lookup, operand: &Scalar) -> bool {
    match (value, operand) {
        (Lookup::Number(number), Scalar::Number(operand)) => number == *operand,
        (Lookup::String(text), Scalar::String(operand)) => text == operand,
        _ => false,
    }
}

fn is_member(value: Lookup, operand: &[Scalar]) -> bool {
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
