use serde::de::Error;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use serde_json::value::RawValue;

pub(crate) const OPS: [&str; 12] = [
    "existing",
    "not_existing",
    "eq",
    "ne",
    "lt",
    "lte",
    "gt",
    "gte",
    "in",
    "contains",
    "not_contains",
    "matches",
];

/// A policy's or a plan entry's condition. The last two refuse what they're compiled for, never the
/// bundle or the plan.
#[derive(Debug, PartialEq)]
pub enum Condition {
    Valid(Predicate),
    /// Its `op` isn't one this reader knows, which a newer Rail Center can publish.
    Unknown(Value),
    /// As sent, since it may not read as a `Value` at all: a number past a double, or deep nesting.
    Invalid(String),
}

impl<'de> Deserialize<'de> for Condition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Kept raw, so what serde_json can't hold fails this condition and not the document.
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        let Ok(value) = serde_json::from_str::<Value>(raw.get()) else {
            return Ok(Self::Invalid(raw.get().to_string()));
        };
        Ok(read_condition(value, raw.get()))
    }
}

// Either spelling of its path; which paths a use reads is checked when compiling.
fn read_condition(value: Value, raw: &str) -> Condition {
    // Not only for `op`: `Predicate` alone would accept a positional array.
    let Some(op) = value["op"].as_str() else {
        return Condition::Invalid(raw.to_string());
    };
    if !OPS.contains(&op) {
        return Condition::Unknown(value);
    }
    match Predicate::deserialize(&value) {
        Ok(predicate) => Condition::Valid(predicate),
        Err(_) => Condition::Invalid(raw.to_string()),
    }
}

/// `{path, op, value}`, tagged on `op`.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Predicate {
    Existing {
        path: Path,
    },
    NotExisting {
        path: Path,
    },
    Eq {
        path: Path,
        value: Scalar,
    },
    Ne {
        path: Path,
        value: Scalar,
    },
    Lt {
        path: Path,
        value: f64,
    },
    Lte {
        path: Path,
        value: f64,
    },
    Gt {
        path: Path,
        value: f64,
    },
    Gte {
        path: Path,
        value: f64,
    },
    In {
        path: Path,
        // Any JSON: an item that isn't a scalar is just a member of nothing.
        value: Vec<Value>,
    },
    Contains {
        path: Path,
        value: Scalar,
    },
    NotContains {
        path: Path,
        value: Scalar,
    },
    Matches {
        path: Path,
        value: String,
    },
}

impl Predicate {
    pub fn path(&self) -> &Path {
        match self {
            Self::Existing { path }
            | Self::NotExisting { path }
            | Self::Eq { path, .. }
            | Self::Ne { path, .. }
            | Self::Lt { path, .. }
            | Self::Lte { path, .. }
            | Self::Gt { path, .. }
            | Self::Gte { path, .. }
            | Self::In { path, .. }
            | Self::Contains { path, .. }
            | Self::NotContains { path, .. }
            | Self::Matches { path, .. } => path,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
    Number(f64),
    String(String),
}

/// What a condition, a context key or a measure reads: a source, then steps into it. Spelled as a
/// dotted string, `<source>.<key>.<key>`, every step a key; or as a list, `["<source>", <step>, ...]`,
/// a number step an index. Which paths a condition reads is checked for its use case when
/// compiling; a record's, when its plan is read.
#[derive(Clone, Debug, PartialEq)]
pub enum Path {
    Ticket(TicketPath),
    Request(RequestPath),
}

impl TryFrom<String> for Path {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let steps: Vec<Value> = text.split('.').map(Value::from).collect();
        Self::from_steps(&steps).ok_or_else(|| format!("path {text:?}"))
    }
}

impl<'de> Deserialize<'de> for Path {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let spelled = Value::deserialize(deserializer)?;
        let path = match &spelled {
            Value::String(text) => Self::try_from(text.clone()).ok(),
            Value::Array(steps) => Self::from_steps(steps),
            _ => None,
        };
        path.ok_or_else(|| D::Error::custom(format!("path {spelled}")))
    }
}

impl Path {
    /// From a list's steps; `None` for a source, a field or a step this reader doesn't know.
    pub fn from_steps(steps: &[Value]) -> Option<Self> {
        let (source, steps) = steps.split_first()?;
        Some(match source.as_str()? {
            "ticket" => Self::Ticket(TicketPath(
                steps
                    .iter()
                    .map(|step| step.as_str().map(str::to_string))
                    .collect::<Option<_>>()?,
            )),
            "request" => Self::Request(RequestPath::parse(steps)?),
            _ => return None,
        })
    }
}

/// What a condition is read for, which decides the paths it may read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConditionUse {
    /// A bundle's policy.
    GatewayPolicy,
    /// A capture plan entry's match.
    CapturePlan,
}

impl ConditionUse {
    pub(crate) fn reads(self, path: &Path) -> bool {
        match self {
            Self::GatewayPolicy => matches!(
                path,
                Path::Ticket(_)
                    | Path::Request(
                        RequestPath::Whole
                            | RequestPath::EndpointKey
                            | RequestPath::Headers
                            | RequestPath::Header(_)
                    )
            ),
            Self::CapturePlan => matches!(
                path,
                Path::Request(
                    RequestPath::Body(_)
                        | RequestPath::Host
                        | RequestPath::Method
                        | RequestPath::Path(None)
                        | RequestPath::RedactedPath(None)
                        | RequestPath::McpMethod
                        | RequestPath::McpTool
                )
            ),
        }
    }
}

/// A path under `ticket`, as its keys. Any path is evaluable, and resolves to absent where the
/// payload holds nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct TicketPath(pub Vec<String>);

/// A path under `request`.
#[derive(Clone, Debug, PartialEq)]
pub enum RequestPath {
    Whole,
    EndpointKey,
    Headers,
    /// Matched exactly, so a name with an upper-case letter never exists.
    Header(String),
    Host,
    Method,
    /// Up to its first `?` or `#`, as received; or one of its segments, from 0:
    /// `["request", "path", <n>]`.
    Path(Option<usize>),
    /// The path redacted, or one of its segments, from 0: `["request", "redacted_path", <n>]`; a
    /// segment that looked like a secret reads `[redacted]`.
    RedactedPath(Option<usize>),
    /// A query parameter's values by its name, or one of them, from 0:
    /// `["request", "query", "<name>", <n>]`.
    Query {
        name: String,
        index: Option<usize>,
    },
    McpMethod,
    McpTool,
    Body(BodyPath),
}

impl RequestPath {
    /// Whether it's read from the body, which a reader may hold only once the request is whole.
    pub(crate) fn reads_the_body(&self) -> bool {
        matches!(self, Self::Body(_) | Self::McpMethod | Self::McpTool)
    }

    /// From the steps past `request`; `None` for any field or step this reader doesn't know.
    fn parse(steps: &[Value]) -> Option<Self> {
        match steps {
            [field, steps @ ..] if field == "body" => {
                let steps = steps.iter().map(BodyStep::read).collect::<Option<_>>()?;
                return Some(Self::Body(BodyPath(steps)));
            }
            [field, index] if field == "path" => {
                return Some(Self::Path(Some(read_index(index)?)));
            }
            [field, index] if field == "redacted_path" => {
                return Some(Self::RedactedPath(Some(read_index(index)?)));
            }
            [field, Value::String(name), index @ ..] if field == "query" && !name.is_empty() => {
                let index = match index {
                    [] => None,
                    [index] => Some(read_index(index)?),
                    _ => return None,
                };
                let name = name.clone();
                return Some(Self::Query { name, index });
            }
            _ => {}
        }
        let keys: Vec<&str> = steps.iter().map(Value::as_str).collect::<Option<_>>()?;
        Some(match keys.as_slice() {
            [] => Self::Whole,
            ["endpoint_key"] => Self::EndpointKey,
            ["headers"] => Self::Headers,
            ["headers", name] => Self::Header(name.to_string()),
            ["host"] => Self::Host,
            ["method"] => Self::Method,
            ["path"] => Self::Path(None),
            ["redacted_path"] => Self::RedactedPath(None),
            ["mcp", "method"] => Self::McpMethod,
            ["mcp", "tool"] => Self::McpTool,
            _ => return None,
        })
    }
}

/// A path under `request.body`, as its steps; none for the whole body.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyPath(pub Vec<BodyStep>);

/// A step into a body: an object's key, or an array's index from 0.
#[derive(Clone, Debug, PartialEq)]
pub enum BodyStep {
    Key(String),
    Index(usize),
}

impl BodyStep {
    fn read(step: &Value) -> Option<Self> {
        match step {
            Value::String(key) => Some(Self::Key(key.clone())),
            index => read_index(index).map(Self::Index),
        }
    }
}

// A whole number from 0, as a list spells an index.
fn read_index(step: &Value) -> Option<usize> {
    usize::try_from(step.as_u64()?).ok()
}
