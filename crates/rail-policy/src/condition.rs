use serde::{Deserialize, Deserializer};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::schema::{RequestPath, TicketPath};

const OPS: [&str; 12] = [
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

/// A policy's condition. The last two refuse the request that reaches them, not the bundle.
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
        // Kept raw, so what serde_json can't hold fails this condition and not the bundle.
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        let Ok(value) = serde_json::from_str::<Value>(raw.get()) else {
            return Ok(Self::Invalid(raw.get().to_string()));
        };
        Ok(read_condition(value, raw.get()))
    }
}

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

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
    Number(f64),
    String(String),
}

/// `<source>.<key>.<key>` on the wire.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub enum Path {
    Ticket(TicketPath),
    Request(RequestPath),
}

impl TryFrom<String> for Path {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let mut parts = text.split('.');
        let source = parts.next();
        let keys: Vec<&str> = parts.collect();
        let path = match source {
            Some("ticket") => TicketPath::parse(&keys).map(Self::Ticket),
            Some("request") => RequestPath::parse(&keys).map(Self::Request),
            _ => None,
        };
        path.ok_or_else(|| format!("path {text:?}"))
    }
}
