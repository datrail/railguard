use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::condition::{OPS, Path, RequestPath, TicketPath};
use crate::sanitize::sanitize;

/// Rail Center nests a condition no deeper than this, so it can render it back out.
const MAX_AUTHORING_DEPTH: usize = 32;

const EXISTENCE_OPS: [&str; 2] = ["existing", "not_existing"];

/// Something wrong with a condition, and who refuses it for that.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConditionProblem {
    /// Where in the condition, from its root; empty for the whole condition.
    pub location: Vec<LocationSegment>,
    pub code: ProblemCode,
    pub scope: ProblemScope,
    /// The operand shape the op takes, for `ProblemCode::OperandShape` only.
    pub expected_shape: Option<OperandShape>,
    pub message: String,
}

/// One step into a condition: an object's key, or a list's index.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum LocationSegment {
    Key(String),
    Index(usize),
}

impl From<&str> for LocationSegment {
    fn from(key: &str) -> Self {
        Self::Key(key.to_string())
    }
}

impl From<usize> for LocationSegment {
    fn from(index: usize) -> Self {
        Self::Index(index)
    }
}

/// A location from keys and indexes: `loc!["value", 1]`.
#[macro_export]
macro_rules! loc {
    ($($segment:expr),* $(,)?) => {
        vec![$($crate::LocationSegment::from($segment)),*]
    };
}

/// Rail Center's refusal codes, plus `Unreadable`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemCode {
    NotAnObject,
    UnknownSource,
    UnknownOp,
    OperandShape,
    UnknownPath,
    OpNotForPath,
    TooDeep,
    /// Not JSON serde_json can hold: a number past a double, deep nesting, a lone surrogate.
    Unreadable,
}

/// `Enforcement`: every reader refuses it, reading the condition as `Invalid`. `Authoring`:
/// only the writer does, since a reader may be on another version.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemScope {
    Enforcement,
    Authoring,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperandShape {
    Number,
    List,
    /// A string or a number.
    Scalar,
    String,
}

/// What a path holds, which decides the ops that can hold for it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    String,
    Number,
    Bool,
    List,
    /// A string, or a list of strings for a header sent with several values.
    Header,
    Object,
}

/// The claims a ticket carries, which rules read: nested objects whose leaves are kinds.
#[derive(Debug)]
pub struct TicketClaimsSchema(BTreeMap<String, SchemaNode>);

#[derive(Debug)]
enum SchemaNode {
    Leaf(Kind),
    Branch(BTreeMap<String, SchemaNode>),
}

impl TicketClaimsSchema {
    /// From an object whose leaves are `"string"`, `"number"`, `"bool"` or `"list"`.
    pub fn from_value(value: &Value) -> Result<Self, String> {
        let Value::Object(fields) = value else {
            return Err("a ticket claims schema is an object".to_string());
        };
        read_branch(fields).map(Self)
    }

    /// `None` where the schema has nothing at `keys`.
    fn get_kind(&self, keys: &[String]) -> Option<Kind> {
        let mut nodes = &self.0;
        let mut kind = Kind::Object;
        for (i, key) in keys.iter().enumerate() {
            match nodes.get(key)? {
                SchemaNode::Branch(next) => nodes = next,
                SchemaNode::Leaf(leaf) if i == keys.len() - 1 => kind = *leaf,
                SchemaNode::Leaf(_) => return None,
            }
        }
        Some(kind)
    }
}

fn read_branch(fields: &Map<String, Value>) -> Result<BTreeMap<String, SchemaNode>, String> {
    fields
        .iter()
        .map(|(key, value)| Ok((key.clone(), read_node(value)?)))
        .collect()
}

fn read_node(value: &Value) -> Result<SchemaNode, String> {
    match value {
        Value::Object(fields) => read_branch(fields).map(SchemaNode::Branch),
        Value::String(kind) => match kind.as_str() {
            "string" => Ok(SchemaNode::Leaf(Kind::String)),
            "number" => Ok(SchemaNode::Leaf(Kind::Number)),
            "bool" => Ok(SchemaNode::Leaf(Kind::Bool)),
            "list" => Ok(SchemaNode::Leaf(Kind::List)),
            _ => Err(format!(
                "kind \"{}\": not string, number, bool or list",
                sanitize(kind)
            )),
        },
        _ => Err("a ticket claims schema's leaves are kinds".to_string()),
    }
}

/// Every problem with a condition given as JSON text, in the order found: first as every reader
/// parses it, then as only Rail Center checks it. None means every reader reads it, and Rail
/// Center would publish it.
pub fn check_condition(
    text: &str,
    ticket_claims_schema: &TicketClaimsSchema,
) -> Vec<ConditionProblem> {
    let condition: Value = match serde_json::from_str(text) {
        Ok(condition) => condition,
        Err(e) => {
            let message = format!(
                "not JSON this reader can hold: {}",
                sanitize(&e.to_string())
            );
            return vec![build_problem(
                loc![],
                ProblemCode::Unreadable,
                ProblemScope::Enforcement,
                message,
            )];
        }
    };
    let Value::Object(fields) = &condition else {
        let message = "not a JSON object".to_string();
        return vec![build_problem(
            loc![],
            ProblemCode::NotAnObject,
            ProblemScope::Enforcement,
            message,
        )];
    };
    let mut problems = Vec::new();
    let parsed = check_grammar(fields, &mut problems);
    check_authoring(&condition, parsed, ticket_claims_schema, &mut problems);
    problems
}

/// What a reader parsed, for the checks only Rail Center makes.
struct Parsed<'a> {
    /// `None` where the op is missing or unknown.
    op: Option<&'a str>,
    /// With its text as sent, or `None` where it doesn't parse.
    path: Option<(&'a str, Path)>,
}

/// As a reader parses a condition: the op, then the path and the operand.
fn check_grammar<'a>(
    fields: &'a Map<String, Value>,
    problems: &mut Vec<ConditionProblem>,
) -> Parsed<'a> {
    let op = fields.get("op").and_then(Value::as_str);
    let known_op = match op {
        None => {
            problems.push(build_problem(
                loc!["op"],
                ProblemCode::UnknownOp,
                ProblemScope::Enforcement,
                "not a string".to_string(),
            ));
            None
        }
        Some(op) if !OPS.contains(&op) => {
            problems.push(build_problem(
                loc!["op"],
                ProblemCode::UnknownOp,
                ProblemScope::Authoring,
                format!("\"{}\": not an op this reader knows", sanitize(op)),
            ));
            None
        }
        Some(op) => Some(op),
    };
    // A reader reads an op it doesn't know as `Unknown`, whatever the rest holds.
    let scope = if op.is_some() && known_op.is_none() {
        ProblemScope::Authoring
    } else {
        ProblemScope::Enforcement
    };
    let path = check_path(fields.get("path"), scope, problems);
    if let Some(op) = known_op
        && let Some(expected) = get_operand_shape(op)
        && !is_shaped(fields.get("value"), expected)
    {
        problems.push(ConditionProblem {
            expected_shape: Some(expected),
            ..build_problem(
                loc!["value"],
                ProblemCode::OperandShape,
                ProblemScope::Enforcement,
                format!("{op} takes a {}", format_shape(expected)),
            )
        });
    }
    Parsed { op: known_op, path }
}

fn check_path<'a>(
    path: Option<&'a Value>,
    scope: ProblemScope,
    problems: &mut Vec<ConditionProblem>,
) -> Option<(&'a str, Path)> {
    let Some(text) = path.and_then(Value::as_str) else {
        let message = "not a string".to_string();
        problems.push(build_problem(
            loc!["path"],
            ProblemCode::UnknownSource,
            scope,
            message,
        ));
        return None;
    };
    let shown = sanitize(text);
    let (code, message) = if !matches!(text.split('.').next(), Some("ticket" | "request")) {
        (
            ProblemCode::UnknownSource,
            format!("\"{shown}\": not under ticket or request"),
        )
    } else {
        match Path::try_from(text.to_string()) {
            Ok(path) => return Some((text, path)),
            // Only a `request` path can fail: any `ticket` path parses.
            Err(_) => (
                ProblemCode::UnknownPath,
                format!("\"{shown}\": names nothing a request carries"),
            ),
        }
    };
    problems.push(build_problem(loc!["path"], code, scope, message));
    None
}

/// What only Rail Center refuses: a path its schema doesn't hold, an op that can't hold for the
/// path, and deep nesting.
fn check_authoring(
    condition: &Value,
    parsed: Parsed,
    ticket_claims_schema: &TicketClaimsSchema,
    problems: &mut Vec<ConditionProblem>,
) {
    if let Some((text, path)) = &parsed.path {
        match get_path_kind(text, path, ticket_claims_schema) {
            Ok(kind) => {
                if let Some(op) = parsed.op
                    && !is_op_for(kind, op)
                {
                    problems.push(build_problem(
                        loc!["op"],
                        ProblemCode::OpNotForPath,
                        ProblemScope::Authoring,
                        format!("{op} can't hold for {}", format_kind(kind)),
                    ));
                }
            }
            Err(message) => problems.push(build_problem(
                loc!["path"],
                ProblemCode::UnknownPath,
                ProblemScope::Authoring,
                message,
            )),
        }
    }
    if get_depth(condition) > MAX_AUTHORING_DEPTH {
        let message = format!("nested deeper than {MAX_AUTHORING_DEPTH} levels");
        problems.push(build_problem(
            loc![],
            ProblemCode::TooDeep,
            ProblemScope::Authoring,
            message,
        ));
    }
}

/// The path's kind, or why Rail Center refuses the path.
fn get_path_kind(
    text: &str,
    path: &Path,
    ticket_claims_schema: &TicketClaimsSchema,
) -> Result<Kind, String> {
    let shown = sanitize(text);
    match path {
        Path::Ticket(TicketPath(keys)) => ticket_claims_schema
            .get_kind(keys)
            .ok_or_else(|| format!("\"{shown}\": names nothing a ticket carries")),
        // A reader reads any such name, and it never exists.
        Path::Request(RequestPath::Header(name))
            if name.is_empty() || name.to_lowercase() != *name =>
        {
            Err(format!(
                "\"{shown}\": a header name is lowercase and not empty"
            ))
        }
        Path::Request(RequestPath::Whole | RequestPath::Headers) => Ok(Kind::Object),
        Path::Request(RequestPath::EndpointKey) => Ok(Kind::String),
        Path::Request(RequestPath::Header(_)) => Ok(Kind::Header),
    }
}

fn build_problem(
    location: Vec<LocationSegment>,
    code: ProblemCode,
    scope: ProblemScope,
    message: String,
) -> ConditionProblem {
    ConditionProblem {
        location,
        code,
        scope,
        expected_shape: None,
        message,
    }
}

/// As `Predicate` reads `value`.
fn get_operand_shape(op: &str) -> Option<OperandShape> {
    match op {
        "eq" | "ne" | "contains" | "not_contains" => Some(OperandShape::Scalar),
        "lt" | "lte" | "gt" | "gte" => Some(OperandShape::Number),
        "in" => Some(OperandShape::List),
        "matches" => Some(OperandShape::String),
        _ => None,
    }
}

fn is_shaped(value: Option<&Value>, shape: OperandShape) -> bool {
    matches!(
        (shape, value),
        (OperandShape::Number, Some(Value::Number(_)))
            | (OperandShape::List, Some(Value::Array(_)))
            | (OperandShape::String, Some(Value::String(_)))
            | (
                OperandShape::Scalar,
                Some(Value::String(_) | Value::Number(_))
            )
    )
}

/// Any other op on a kind is a rule that can never hold.
fn is_op_for(kind: Kind, op: &str) -> bool {
    let ops: &[&str] = match kind {
        Kind::Object | Kind::Bool => &[],
        Kind::String => &["eq", "ne", "in", "matches"],
        Kind::Number => &["eq", "ne", "lt", "lte", "gt", "gte", "in"],
        Kind::List | Kind::Header => &["eq", "ne", "in", "matches", "contains", "not_contains"],
    };
    EXISTENCE_OPS.contains(&op) || ops.contains(&op)
}

fn format_shape(shape: OperandShape) -> &'static str {
    match shape {
        OperandShape::Number => "number",
        OperandShape::List => "list",
        OperandShape::Scalar => "string or number",
        OperandShape::String => "string",
    }
}

fn format_kind(kind: Kind) -> &'static str {
    match kind {
        Kind::String => "a string",
        Kind::Number => "a number",
        Kind::Bool => "a boolean",
        Kind::List => "a list",
        Kind::Header => "a header",
        Kind::Object => "an object",
    }
}

/// As Rail Center counts it: the condition is level 1. Iterative, since depth is the point.
fn get_depth(condition: &Value) -> usize {
    let mut deepest = 0;
    let mut stack = vec![(condition, 1)];
    while let Some((value, level)) = stack.pop() {
        deepest = deepest.max(level);
        match value {
            Value::Object(fields) => stack.extend(fields.values().map(|v| (v, level + 1))),
            Value::Array(items) => stack.extend(items.iter().map(|v| (v, level + 1))),
            _ => {}
        }
    }
    deepest
}
