//! A capture plan as Rail Center serves it: its shape, every object closed, then what the field
//! reference adds to it. Its values are as Rail Center writes them.

use std::collections::HashSet;
use std::fmt;
use std::ops::RangeInclusive;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::agent::AgentId;
use crate::condition::{
    BodyStep, Condition, ConditionUse, Path, RequestPath, Spelling, read_condition,
};
use crate::sanitize::sanitize;
use crate::schema_version::SchemaVersion;

const CATCH_ALL: &str = "catch_all";

// The proxy's ceilings on what Rail Center may ask it to hold; the schema has only minimums.
const WINDOW_SECONDS: RangeInclusive<u32> = 60..=86_400;
const MAX_GROUPS_PER_WINDOW: u32 = 100_000;
const MAX_STEPS_PER_WINDOW: u32 = 1_000_000;
const MAX_UNSENT_WINDOWS: u32 = 1_000;

const MAX_ENDPOINT_ID_CHARS: usize = 64;
const MAX_NAME_CHARS: usize = 32;

/// The body of Rail Center's `GET /v1/capture-plan`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema_version: SchemaVersion,
    /// Changes with every field but itself and `generated_at`. It orders nothing.
    pub content_hash: String,
    /// RFC 3339 in UTC, with a `Z`.
    pub generated_at: String,
    pub agent: AgentId,
    pub plan_version: u64,
    /// The `REDACTION_VERSION` its paths were redacted with.
    pub redaction_version: u64,
    pub window_seconds: u32,
    pub budget: Budget,
    /// Tried in this order.
    pub entries: Vec<Entry>,
    pub catch_all: CatchAll,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub max_groups_per_window: u32,
    /// Present: the order of calls is recorded too, up to this many steps.
    #[serde(default, deserialize_with = "present")]
    pub max_steps_per_window: Option<u32>,
    pub on_max_groups: OnMaxGroups,
    pub max_unsent_windows: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnMaxGroups {
    CoarsenToCatchAll,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    /// Rail Center's id for the endpoint, never read.
    #[serde(default, deserialize_with = "present")]
    pub endpoint_id: Option<String>,
    /// Read as written; checked when compiled, so one that fails refuses its entry, not the plan.
    #[serde(rename = "match", deserialize_with = "read_conditions")]
    pub conditions: Vec<Condition>,
    pub record: Record,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub context_keys: Vec<ContextKey>,
    #[serde(rename = "measure")]
    pub measures: Vec<Measure>,
}

/// How every call no entry takes is counted.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatchAll {
    /// Always `catch_all`.
    pub id: String,
    pub record: CatchAllRecord,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatchAllRecord {
    pub context_keys: Vec<CatchAllKey>,
    /// The three every record holds, and no other.
    #[serde(rename = "measure")]
    pub measures: Vec<Measure>,
}

#[derive(Debug, PartialEq)]
pub enum ContextKey {
    Method,
    Named {
        name: String,
        path: Path,
        /// Copied into each record, never read.
        field_type: String,
    },
}

#[derive(Debug, PartialEq)]
pub enum CatchAllKey {
    Host,
    PathPrefix { segments: u32 },
    Method,
    Operation,
}

#[derive(Debug, PartialEq)]
pub enum Measure {
    CallCount,
    ErrorCount,
    /// The whole body's size, as sent.
    RequestBytes,
    Named {
        name: String,
        combine: Combine,
        function: Function,
        paths: Vec<Path>,
    },
}

/// How a measure's per-call numbers add up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Combine {
    Sum,
}

/// What a call adds to a measure, from the values its paths find.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Function {
    Size,
}

/// Why a plan can't be used. It is refused whole.
#[derive(Debug)]
pub struct InvalidPlan(String);

impl fmt::Display for InvalidPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid capture plan: {}", self.0)
    }
}

impl std::error::Error for InvalidPlan {}

/// Reads a plan and checks it against the field reference. Its `schema_version`'s major, its
/// agent and its `redaction_version` are the caller's to compare.
pub fn parse_plan(json: &str) -> Result<Plan, InvalidPlan> {
    let refuse = |why: &str| InvalidPlan(sanitize(why).to_string());
    let plan: Plan = serde_json::from_str(json).map_err(|error| refuse(&error.to_string()))?;
    check_plan(&plan).map_err(|why| refuse(&why))?;
    Ok(plan)
}

fn check_plan(plan: &Plan) -> Result<(), String> {
    if !is_utc_time(&plan.generated_at) {
        return Err("generated_at isn't an RFC 3339 time in UTC, with a Z".to_owned());
    }
    if plan.plan_version == 0 || plan.redaction_version == 0 {
        return Err("plan_version and redaction_version are from 1".to_owned());
    }
    if !WINDOW_SECONDS.contains(&plan.window_seconds) {
        return Err(format!(
            "window_seconds is outside {} to {}",
            WINDOW_SECONDS.start(),
            WINDOW_SECONDS.end()
        ));
    }
    check_budget(&plan.budget)?;
    let mut ids = HashSet::new();
    for entry in &plan.entries {
        if !ids.insert(entry.id.as_str()) {
            return Err(format!("entry {} appears twice", entry.id));
        }
        check_entry(entry).map_err(|why| format!("entry {}: {why}", entry.id))?;
    }
    check_catch_all(&plan.catch_all).map_err(|why| format!("catch_all: {why}"))
}

fn check_budget(budget: &Budget) -> Result<(), String> {
    let groups = budget.max_groups_per_window;
    if !(1..=MAX_GROUPS_PER_WINDOW).contains(&groups) {
        return Err(format!(
            "max_groups_per_window is outside 1 to {MAX_GROUPS_PER_WINDOW}"
        ));
    }
    if let Some(steps) = budget.max_steps_per_window
        && !(groups..=MAX_STEPS_PER_WINDOW).contains(&steps)
    {
        return Err(format!(
            "max_steps_per_window is outside max_groups_per_window to {MAX_STEPS_PER_WINDOW}"
        ));
    }
    if budget.max_unsent_windows > MAX_UNSENT_WINDOWS {
        return Err(format!("max_unsent_windows is above {MAX_UNSENT_WINDOWS}"));
    }
    Ok(())
}

fn check_entry(entry: &Entry) -> Result<(), String> {
    if entry.id.is_empty() || entry.id == CATCH_ALL {
        return Err(format!("its id is empty or {CATCH_ALL}"));
    }
    if let Some(endpoint_id) = &entry.endpoint_id
        && !(1..=MAX_ENDPOINT_ID_CHARS).contains(&endpoint_id.chars().count())
    {
        return Err(format!(
            "its endpoint_id isn't 1 to {MAX_ENDPOINT_ID_CHARS} characters"
        ));
    }
    if entry.conditions.is_empty() {
        return Err("its match is empty".to_owned());
    }
    let keys = entry.record.context_keys.iter().map(|key| match key {
        ContextKey::Method => "method",
        ContextKey::Named { name, .. } => name,
    });
    unique("context key", keys)?;
    check_measures(&entry.record.measures)
}

fn check_catch_all(catch_all: &CatchAll) -> Result<(), String> {
    if catch_all.id != CATCH_ALL {
        return Err(format!("its id isn't {CATCH_ALL}"));
    }
    let keys = &catch_all.record.context_keys;
    let names = keys.iter().map(|key| match key {
        CatchAllKey::Host => "host",
        CatchAllKey::PathPrefix { .. } => "path_prefix",
        CatchAllKey::Method => "method",
        CatchAllKey::Operation => "operation",
    });
    unique("context key", names)?;
    if !keys.contains(&CatchAllKey::Host) {
        return Err("no host context key".to_owned());
    }
    let measures = &catch_all.record.measures;
    if let Some(Measure::Named { name, .. }) = measures
        .iter()
        .find(|measure| matches!(measure, Measure::Named { .. }))
    {
        return Err(format!("measure {name}: only the built-in ones"));
    }
    check_measures(measures)
}

// A set by name, holding the three every record does.
fn check_measures(measures: &[Measure]) -> Result<(), String> {
    let names = measures.iter().map(|measure| match measure {
        Measure::CallCount => "call_count",
        Measure::ErrorCount => "error_count",
        Measure::RequestBytes => "request_bytes",
        Measure::Named { name, .. } => name,
    });
    unique("measure", names)?;
    for (built_in, name) in [
        (Measure::CallCount, "call_count"),
        (Measure::ErrorCount, "error_count"),
        (Measure::RequestBytes, "request_bytes"),
    ] {
        if !measures.contains(&built_in) {
            return Err(format!("no {name} measure"));
        }
    }
    Ok(())
}

fn unique<'a>(what: &str, names: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(format!("{what} {name} appears twice"));
        }
    }
    Ok(())
}

// `YYYY-MM-DDTHH:MM:SS`, fractional seconds or none, then `Z`.
fn is_utc_time(text: &str) -> bool {
    let Some(text) = text.strip_suffix('Z') else {
        return false;
    };
    let (whole, fraction) = text.split_once('.').unwrap_or((text, "0"));
    let shape = b"dddd-dd-ddTdd:dd:dd";
    whole.len() == shape.len()
        && whole
            .bytes()
            .zip(shape)
            .all(|(byte, expected)| match expected {
                b'd' => byte.is_ascii_digit(),
                other => byte == *other,
            })
        && !fraction.is_empty()
        && fraction.bytes().all(|byte| byte.is_ascii_digit())
}

// An optional field is left out, never `null`.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

// Raw, so what serde_json can't hold fails a condition, not the plan.
fn read_conditions<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Condition>, D::Error> {
    let conditions = Vec::<Box<RawValue>>::deserialize(deserializer)?;
    conditions
        .iter()
        .map(|raw| {
            // A raw value holds no whitespace around it.
            if !raw.get().starts_with('{') {
                return Err(D::Error::custom("a condition isn't an object"));
            }
            Ok(read_condition(raw, Spelling::Plan))
        })
        .collect()
}

impl<'de> Deserialize<'de> for ContextKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = Map::deserialize(deserializer)?;
        read_context_key(&mut fields).map_err(D::Error::custom)
    }
}

// A key's fields depend on its name.
fn read_context_key(fields: &mut Map<String, Value>) -> Result<ContextKey, String> {
    let name = take_name(fields, "context key")?;
    let at = format!("context key {name}");
    match name.as_str() {
        "method" => {
            only_its_own(fields, &at)?;
            Ok(ContextKey::Method)
        }
        "host" | "path_prefix" | "operation" => Err(format!("{at}: the catch-all's alone")),
        _ => {
            let path = take(fields, "path", &at)?;
            let field_type = take(fields, "field_type", &at)?;
            only_its_own(fields, &at)?;
            let Value::String(field_type) = field_type else {
                return Err(format!("{at}: its field_type isn't a string"));
            };
            if !is_name(&field_type) {
                return Err(format!("{at}: its field_type {NOT_A_NAME}"));
            }
            Ok(ContextKey::Named {
                path: read_record_path(&path).map_err(|why| format!("{at}: {why}"))?,
                name,
                field_type,
            })
        }
    }
}

impl<'de> Deserialize<'de> for CatchAllKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = Map::deserialize(deserializer)?;
        read_catch_all_key(&mut fields).map_err(D::Error::custom)
    }
}

fn read_catch_all_key(fields: &mut Map<String, Value>) -> Result<CatchAllKey, String> {
    let name = take_name(fields, "context key")?;
    let at = format!("context key {name}");
    let key = match name.as_str() {
        "host" => CatchAllKey::Host,
        "method" => CatchAllKey::Method,
        "operation" => CatchAllKey::Operation,
        "path_prefix" => {
            let segments = take(fields, "segments", &at)?;
            match segments
                .as_u64()
                .and_then(|segments| u32::try_from(segments).ok())
            {
                Some(segments @ 1..) => CatchAllKey::PathPrefix { segments },
                _ => return Err(format!("{at}: its segments aren't a whole number from 1")),
            }
        }
        _ => return Err(format!("{at}: not a built-in one")),
    };
    only_its_own(fields, &at)?;
    Ok(key)
}

impl<'de> Deserialize<'de> for Measure {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = Map::deserialize(deserializer)?;
        read_measure(&mut fields).map_err(D::Error::custom)
    }
}

// A measure's fields depend on its name.
fn read_measure(fields: &mut Map<String, Value>) -> Result<Measure, String> {
    let name = take_name(fields, "measure")?;
    let at = format!("measure {name}");
    let built_in = match name.as_str() {
        "call_count" => Some(Measure::CallCount),
        "error_count" => Some(Measure::ErrorCount),
        _ => None,
    };
    if let Some(built_in) = built_in {
        only_its_own(fields, &at)?;
        return Ok(built_in);
    }
    let combine = take(fields, "type", &at)?;
    let function = take(fields, "function", &at)?;
    let paths = take(fields, "paths", &at)?;
    only_its_own(fields, &at)?;
    if name == "request_bytes" {
        let whole_body = json!([["request", "body"]]);
        if (combine, function, paths) != (json!("sum"), json!("size"), whole_body) {
            return Err(format!(
                "{at}: not a sum of size over [[\"request\", \"body\"]]"
            ));
        }
        return Ok(Measure::RequestBytes);
    }
    let unlisted = |field: &str| format!("{at}: its {field} is none the reference lists");
    let combine = Combine::deserialize(combine).map_err(|_| unlisted("type"))?;
    let function = Function::deserialize(function).map_err(|_| unlisted("function"))?;
    let paths = match paths {
        Value::Array(paths) if !paths.is_empty() => paths,
        _ => return Err(format!("{at}: its paths aren't a list of one or more")),
    };
    let paths = paths
        .iter()
        .map(read_record_path)
        .collect::<Result<_, _>>()
        .map_err(|why| format!("{at}: {why}"))?;
    Ok(Measure::Named {
        name,
        combine,
        function,
        paths,
    })
}

// A built-in name, or one synthesis names.
fn take_name(fields: &mut Map<String, Value>, what: &str) -> Result<String, String> {
    let Some(Value::String(name)) = fields.remove("name") else {
        return Err(format!("a {what} with no name that's a string"));
    };
    let built_in = [
        "host",
        "path_prefix",
        "method",
        "operation",
        "call_count",
        "error_count",
        "request_bytes",
    ];
    if built_in.contains(&name.as_str()) || is_name(&name) {
        Ok(name)
    } else {
        Err(format!("{what} {name}: its name {NOT_A_NAME}"))
    }
}

fn take(fields: &mut Map<String, Value>, field: &str, at: &str) -> Result<Value, String> {
    fields
        .remove(field)
        .ok_or_else(|| format!("{at}: no {field}"))
}

// What's left once its own fields are taken.
fn only_its_own(fields: &Map<String, Value>, at: &str) -> Result<(), String> {
    match fields.keys().next() {
        Some(field) => Err(format!("{at}: a field {field} it doesn't have")),
        None => Ok(()),
    }
}

const NOT_A_NAME: &str = "isn't lowercase letters, digits and _, from a letter, up to 32";

fn is_name(text: &str) -> bool {
    let mut bytes = text.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && text.len() <= MAX_NAME_CHARS
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

// A context key's or a measure's: into the body, the query or the redacted path's segments.
fn read_record_path(steps: &Value) -> Result<Path, String> {
    let path = steps
        .as_array()
        .and_then(|steps| Path::from_steps(steps))
        .filter(|path| ConditionUse::PlanRecord.reads(path))
        .ok_or("a path not into the body, the query or a redacted path segment")?;
    if let Path::Request(RequestPath::Body(body)) = &path
        && body
            .0
            .iter()
            .any(|step| matches!(step, BodyStep::Key(key) if key.is_empty()))
    {
        return Err("a path with an empty key".to_owned());
    }
    Ok(path)
}
