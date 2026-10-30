use serde::Deserialize;
use serde_json::Value;

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
#[derive(Debug, PartialEq, Deserialize)]
#[serde(from = "Value")]
pub enum Condition {
    Valid(Predicate),
    /// Its `op` isn't one this reader knows, which a newer Rail Center can publish.
    Unknown(Value),
    Invalid(Value),
}

impl From<Value> for Condition {
    fn from(value: Value) -> Self {
        // Not only for `op`: `Predicate` alone would accept a positional array.
        let Some(op) = value["op"].as_str() else {
            return Self::Invalid(value);
        };
        if !OPS.contains(&op) {
            return Self::Unknown(value);
        }
        match Predicate::deserialize(&value) {
            Ok(predicate) => Self::Valid(predicate),
            Err(_) => Self::Invalid(value),
        }
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

#[derive(Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
    Number(f64),
    String(String),
}

/// `<source>.<key>.<key>` on the wire.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct Path {
    pub source: Source,
    pub keys: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum Source {
    Ticket,
    Request,
}

impl TryFrom<String> for Path {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let mut parts = text.split('.');
        let source = match parts.next() {
            Some("ticket") => Source::Ticket,
            Some("request") => Source::Request,
            _ => return Err(format!("path {text:?}")),
        };
        let keys: Vec<&str> = parts.collect();
        // A claim this reader doesn't know would read as absent, so a rule on it wouldn't fire.
        if source == Source::Ticket && !is_ticket_path(&keys) {
            return Err(format!("path {text:?}"));
        }
        let keys = keys.into_iter().map(str::to_string).collect();
        Ok(Self { source, keys })
    }
}

fn is_ticket_path(keys: &[&str]) -> bool {
    matches!(
        keys,
        [] | ["id"]
            | ["agent_id"]
            | ["sandbox_id"]
            | ["sandbox_type"]
            | ["environment_fingerprint"]
            | ["skills"]
            | ["posture"]
            | ["posture", "overall"]
            | ["posture", "overall", "score" | "level"]
            | ["posture", "categories"]
            | ["posture", "categories", _]
            | ["posture", "categories", _, "score" | "capped"]
            | ["alignment"]
            | ["alignment", "drifted"]
            | ["alignment", "baseline"]
            | ["alignment", "baseline", "id" | "label"]
    )
}
