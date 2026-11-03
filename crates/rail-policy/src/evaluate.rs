use serde_json::Value;
use uuid::Uuid;

use crate::condition::{Condition, Path, Predicate, Scalar};
use crate::environment::{Endpoint, Environment};
use crate::schema::{RequestPath, TicketPath};

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

    pub fn steps(&self) -> u64 {
        self.steps
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
#[derive(Debug, PartialEq)]
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
enum Lookup<'a> {
    Absent,
    Str(&'a str),
    /// A string on the wire, kept as parsed.
    Uuid(Uuid),
    Number(f64),
    /// Never compared: `true == 1` in some languages and not in others.
    Bool,
    List(&'a [String]),
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

pub fn evaluate_condition(
    condition: &CompiledCondition,
    environment: &Environment,
    budget: &mut Budget,
) -> Result<bool, Refusal> {
    budget.charge(1)?;
    let Node::Test(path, test) = &condition.0 else {
        return Err(Refusal::Uninterpretable);
    };
    let value = resolve(path, environment);
    apply(test, &value, budget)
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

fn resolve_ticket<'a>(path: &TicketPath, environment: &Environment<'a>) -> Lookup<'a> {
    let Some(claims) = environment.ticket else {
        return Lookup::Absent;
    };
    let category = |name: &str| claims.posture.categories.get(name);
    match path {
        TicketPath::Whole
        | TicketPath::Posture
        | TicketPath::Overall
        | TicketPath::Categories
        | TicketPath::Alignment => Lookup::Object,
        TicketPath::Id => Lookup::Uuid(claims.id),
        TicketPath::AgentId => Lookup::Uuid(claims.agent_id),
        TicketPath::SandboxId => Lookup::Str(&claims.sandbox_id),
        TicketPath::SandboxType => claims
            .sandbox_type
            .as_deref()
            .map_or(Lookup::Absent, Lookup::Str),
        TicketPath::EnvironmentFingerprint => Lookup::Str(&claims.environment_fingerprint),
        TicketPath::Skills => Lookup::List(&claims.skills),
        TicketPath::OverallScore => Lookup::Number(claims.posture.overall.score),
        TicketPath::OverallLevel => Lookup::Str(claims.posture.overall.level.as_str()),
        TicketPath::Category(name) => category(name).map_or(Lookup::Absent, |_| Lookup::Object),
        TicketPath::CategoryScore(name) => category(name)
            .and_then(|category| category.score)
            .map_or(Lookup::Absent, Lookup::Number),
        TicketPath::CategoryCapped(name) => category(name).map_or(Lookup::Absent, |_| Lookup::Bool),
        TicketPath::Drifted => Lookup::List(&claims.alignment.drifted),
        TicketPath::Baseline => claims
            .alignment
            .baseline
            .as_ref()
            .map_or(Lookup::Absent, |_| Lookup::Object),
        TicketPath::BaselineId => claims
            .alignment
            .baseline
            .as_ref()
            .map_or(Lookup::Absent, |baseline| Lookup::Uuid(baseline.id)),
        TicketPath::BaselineLabel => claims
            .alignment
            .baseline
            .as_ref()
            .map_or(Lookup::Absent, |baseline| Lookup::Str(&baseline.label)),
    }
}

fn resolve_request<'a>(path: &RequestPath, environment: &Environment<'a>) -> Lookup<'a> {
    match path {
        RequestPath::Whole | RequestPath::Headers => Lookup::Object,
        RequestPath::EndpointKey => match environment.endpoint {
            Endpoint::Resolved(key) => Lookup::Str(key.as_str()),
            Endpoint::Keyless | Endpoint::Unrecognised => Lookup::Absent,
        },
        RequestPath::Header(name) => match environment.headers.get(name) {
            None => Lookup::Absent,
            Some([value]) => Lookup::Str(value),
            Some(values) => Lookup::List(values),
        },
    }
}

fn apply(test: &Test, value: &Lookup, budget: &mut Budget) -> Result<bool, Refusal> {
    if let Lookup::List(items) = value {
        budget.charge(items.len() as u64)?;
    }
    Ok(match (test, value) {
        (Test::Existing, value) => !matches!(value, Lookup::Absent),
        (Test::NotExisting, value) => matches!(value, Lookup::Absent),
        // On a list, `eq` needs every value to equal, and `ne` any value to differ.
        (Test::Eq(operand), Lookup::List(items)) => {
            !items.is_empty() && items.iter().all(|item| equals(&Lookup::Str(item), operand))
        }
        (Test::Eq(operand), value) => equals(value, operand),
        (Test::Ne(operand), Lookup::List(items)) => {
            !items.is_empty()
                && items
                    .iter()
                    .any(|item| !equals(&Lookup::Str(item), operand))
        }
        (Test::Ne(operand), value) => is_scalar(value) && !equals(value, operand),
        (Test::Lt(operand), Lookup::Number(number)) => number < operand,
        (Test::Lte(operand), Lookup::Number(number)) => number <= operand,
        (Test::Gt(operand), Lookup::Number(number)) => number > operand,
        (Test::Gte(operand), Lookup::Number(number)) => number >= operand,
        (Test::In(operand), Lookup::List(items)) => {
            budget.charge((items.len() as u64).saturating_mul(operand.len() as u64))?;
            items
                .iter()
                .any(|item| is_member(&Lookup::Str(item), operand))
        }
        (Test::In(operand), value) => {
            budget.charge(operand.len() as u64)?;
            is_member(value, operand)
        }
        (Test::Contains(operand), Lookup::List(items)) => {
            items.iter().any(|item| equals(&Lookup::Str(item), operand))
        }
        (Test::NotContains(operand), Lookup::List(items)) => {
            !items.iter().any(|item| equals(&Lookup::Str(item), operand))
        }
        (Test::Matches(glob), Lookup::List(items)) => {
            let mut matched = false;
            for item in *items {
                if glob.matches(item, budget)? {
                    matched = true;
                    break;
                }
            }
            matched
        }
        (Test::Matches(glob), value) => match read_text(value, |text| glob.matches(text, budget)) {
            Some(matched) => matched?,
            None => false,
        },
        _ => false,
    })
}

/// A string or a number: the only kinds that compare, and only with their own kind.
fn is_scalar(value: &Lookup) -> bool {
    matches!(value, Lookup::Str(_) | Lookup::Uuid(_) | Lookup::Number(_))
}

fn equals(value: &Lookup, operand: &Scalar) -> bool {
    match (value, operand) {
        (Lookup::Number(number), Scalar::Number(operand)) => number == operand,
        (value, Scalar::String(operand)) => {
            read_text(value, |text| text == operand).unwrap_or(false)
        }
        _ => false,
    }
}

fn is_member(value: &Lookup, operand: &[Scalar]) -> bool {
    operand.iter().any(|item| equals(value, item))
}

/// The value as text, if it is a string on the wire.
fn read_text<R>(value: &Lookup, read: impl FnOnce(&str) -> R) -> Option<R> {
    match value {
        Lookup::Str(text) => Some(read(text)),
        Lookup::Uuid(id) => {
            let mut buffer = [0; uuid::fmt::Hyphenated::LENGTH];
            Some(read(id.hyphenated().encode_lower(&mut buffer)))
        }
        _ => None,
    }
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
