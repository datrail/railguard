use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::condition::{OPS, RequestPath};
use crate::sanitize::sanitize;

/// Rail Center nests a condition no deeper than this, so it can render it back out.
const MAX_AUTHORING_DEPTH: usize = 32;

const EXISTENCE_OPS: [&str; 2] = ["existing", "not_existing"];

/// Something wrong with a condition, and who refuses it for that.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Problem {
    /// Where in the condition, from its root; empty for the whole condition.
    pub location: Vec<Segment>,
    pub code: ProblemCode,
    pub scope: Scope,
    /// The operand shape the op takes, for `OperandShape` only.
    pub expected: Option<OperandShape>,
    pub message: String,
}

/// One step into a condition: an object's key, or a list's index.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Segment {
    Key(String),
    Index(usize),
}

impl From<&str> for Segment {
    fn from(key: &str) -> Self {
        Self::Key(key.to_string())
    }
}

impl From<usize> for Segment {
    fn from(index: usize) -> Self {
        Self::Index(index)
    }
}

/// A location from keys and indexes: `loc!["value", 1]`.
#[macro_export]
macro_rules! loc {
    ($($segment:expr),* $(,)?) => {
        vec![$($crate::Segment::from($segment)),*]
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
pub enum Scope {
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
    Boolean,
    List,
    /// A string, or a list of strings for a header sent with several values.
    Header,
    Object,
}

/// What a ticket carries: nested objects whose leaves are kinds.
#[derive(Debug)]
pub struct TicketSchema(BTreeMap<String, SchemaNode>);

#[derive(Debug)]
enum SchemaNode {
    Leaf(Kind),
    Branch(BTreeMap<String, SchemaNode>),
}

impl TicketSchema {
    /// From an object whose leaves are `"string"`, `"number"`, `"boolean"` or `"list"`.
    pub fn from_value(value: &Value) -> Result<Self, String> {
        match read_node(value)? {
            SchemaNode::Branch(nodes) => Ok(Self(nodes)),
            SchemaNode::Leaf(_) => Err("a ticket schema is an object".to_string()),
        }
    }

    /// `None` where the schema has nothing at `keys`.
    fn get_kind(&self, keys: &[&str]) -> Option<Kind> {
        let mut nodes = &self.0;
        let mut kind = Kind::Object;
        for (i, key) in keys.iter().enumerate() {
            match nodes.get(*key)? {
                SchemaNode::Branch(next) => nodes = next,
                SchemaNode::Leaf(leaf) if i == keys.len() - 1 => kind = *leaf,
                SchemaNode::Leaf(_) => return None,
            }
        }
        Some(kind)
    }
}

fn read_node(value: &Value) -> Result<SchemaNode, String> {
    match value {
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| Ok((key.clone(), read_node(value)?)))
            .collect::<Result<_, String>>()
            .map(SchemaNode::Branch),
        Value::String(kind) => match kind.as_str() {
            "string" => Ok(SchemaNode::Leaf(Kind::String)),
            "number" => Ok(SchemaNode::Leaf(Kind::Number)),
            "boolean" => Ok(SchemaNode::Leaf(Kind::Boolean)),
            "list" => Ok(SchemaNode::Leaf(Kind::List)),
            _ => Err(format!(
                "kind \"{}\": not string, number, boolean or list",
                sanitize(kind)
            )),
        },
        _ => Err("a ticket schema's leaves are kinds".to_string()),
    }
}

/// Every problem with a condition given as JSON text, in the order path, op, value. None means
/// every reader reads it, and Rail Center would publish it.
pub fn check_condition(text: &str, ticket_schema: &TicketSchema) -> Vec<Problem> {
    let condition: Value = match serde_json::from_str(text) {
        Ok(condition) => condition,
        Err(e) => {
            let message = format!(
                "not JSON this reader can hold: {}",
                sanitize(&e.to_string())
            );
            return vec![problem(
                loc![],
                ProblemCode::Unreadable,
                Scope::Enforcement,
                message,
            )];
        }
    };
    let Value::Object(fields) = &condition else {
        let message = "not a JSON object".to_string();
        return vec![problem(
            loc![],
            ProblemCode::NotAnObject,
            Scope::Enforcement,
            message,
        )];
    };
    let op = fields.get("op").and_then(Value::as_str);
    // A reader reads an op it doesn't know as `Unknown`, whatever the rest holds.
    let op_unknown = op.is_some_and(|op| !OPS.contains(&op));
    let scope = if op_unknown {
        Scope::Authoring
    } else {
        Scope::Enforcement
    };
    let mut problems = Vec::new();
    let kind = check_path(fields.get("path"), ticket_schema, scope, &mut problems);
    match op {
        None => problems.push(problem(
            loc!["op"],
            ProblemCode::UnknownOp,
            Scope::Enforcement,
            "not a string".to_string(),
        )),
        Some(op) if op_unknown => problems.push(problem(
            loc!["op"],
            ProblemCode::UnknownOp,
            Scope::Authoring,
            format!("\"{}\": not an op this reader knows", sanitize(op)),
        )),
        Some(op) => check_operand(op, fields, kind, &mut problems),
    }
    if get_depth(&condition) > MAX_AUTHORING_DEPTH {
        let message = format!("nested deeper than {MAX_AUTHORING_DEPTH} levels");
        problems.push(problem(
            loc![],
            ProblemCode::TooDeep,
            Scope::Authoring,
            message,
        ));
    }
    problems
}

/// The path's kind, or `None` where the path has a problem.
fn check_path(
    path: Option<&Value>,
    ticket_schema: &TicketSchema,
    scope: Scope,
    problems: &mut Vec<Problem>,
) -> Option<Kind> {
    let Some(path) = path.and_then(Value::as_str) else {
        let message = "not a string".to_string();
        problems.push(problem(
            loc!["path"],
            ProblemCode::UnknownSource,
            scope,
            message,
        ));
        return None;
    };
    let mut parts = path.split('.');
    let root = parts.next();
    let keys: Vec<&str> = parts.collect();
    let shown = sanitize(path);
    let (code, problem_scope, message) = match root {
        Some("ticket") => match ticket_schema.get_kind(&keys) {
            Some(kind) => return Some(kind),
            None => (
                ProblemCode::UnknownPath,
                Scope::Authoring,
                format!("\"{shown}\": names nothing a ticket carries"),
            ),
        },
        Some("request") => match RequestPath::parse(&keys) {
            // A reader reads any such name, and it never exists.
            Some(RequestPath::Header(name)) if name.is_empty() || name.to_lowercase() != name => (
                ProblemCode::UnknownPath,
                Scope::Authoring,
                format!("\"{shown}\": a header name is lowercase and not empty"),
            ),
            Some(RequestPath::Whole | RequestPath::Headers) => return Some(Kind::Object),
            Some(RequestPath::EndpointKey) => return Some(Kind::String),
            Some(RequestPath::Header(_)) => return Some(Kind::Header),
            None => (
                ProblemCode::UnknownPath,
                scope,
                format!("\"{shown}\": names nothing a request carries"),
            ),
        },
        _ => (
            ProblemCode::UnknownSource,
            scope,
            format!("\"{shown}\": not under ticket or request"),
        ),
    };
    problems.push(problem(loc!["path"], code, problem_scope, message));
    None
}

fn check_operand(
    op: &str,
    fields: &Map<String, Value>,
    kind: Option<Kind>,
    problems: &mut Vec<Problem>,
) {
    if let Some(kind) = kind
        && !is_op_for(kind, op)
    {
        problems.push(problem(
            loc!["op"],
            ProblemCode::OpNotForPath,
            Scope::Authoring,
            format!("{op} can't hold for {}", format_kind(kind)),
        ));
    }
    if let Some(expected) = get_operand_shape(op)
        && !is_shaped(fields.get("value"), expected)
    {
        problems.push(Problem {
            expected: Some(expected),
            ..problem(
                loc!["value"],
                ProblemCode::OperandShape,
                Scope::Enforcement,
                format!("{op} takes a {}", format_shape(expected)),
            )
        });
    }
}

fn problem(location: Vec<Segment>, code: ProblemCode, scope: Scope, message: String) -> Problem {
    Problem {
        location,
        code,
        scope,
        expected: None,
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
        Kind::Object | Kind::Boolean => &[],
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
        Kind::Boolean => "a boolean",
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
