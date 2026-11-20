//! A capture plan as Rail Center serves it: read once and checked whole, its values as written.

use std::collections::{BTreeMap, HashSet};
use std::ops::RangeInclusive;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::agent::Agent;
use crate::condition::{
    BodyStep, Condition, ConditionUse, Path, RequestPath, Spelling, read_condition,
};
use crate::redaction::REDACTION_VERSION;

const SCHEMA_VERSION: &str = "1.0";

const CATCH_ALL: &str = "catch_all";

const FIELDS: [&str; 10] = [
    "schema_version",
    "content_hash",
    "generated_at",
    "agent",
    "plan_version",
    "redaction_version",
    "window_seconds",
    "budget",
    "entries",
    "catch_all",
];

// The proxy's ceilings on what Rail Center may ask it to hold; the schema has only minimums.
const WINDOW_SECONDS: RangeInclusive<u32> = 60..=86_400;
const MAX_GROUPS_PER_WINDOW: u32 = 100_000;
const MAX_STEPS_PER_WINDOW: u32 = 1_000_000;
const MAX_UNSENT_WINDOWS: u32 = 1_000;

const MAX_ENDPOINT_ID_CHARS: usize = 64;

/// An answer that isn't a plan for this proxy, so what's held is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    NotJson,
    /// No `content_hash` reading as `sha256:` and 64 lowercase hex characters.
    NoContentHash,
    /// Another agent's, or one naming no agent.
    OtherAgent,
}

/// A plan as read: its hash and version, whatever else it holds, and what the proxy makes of it.
#[derive(Debug)]
pub struct PlanReading {
    pub content_hash: ContentHash,
    /// Where it reads as a whole number from 1.
    pub plan_version: Option<u64>,
    pub state: PlanState,
}

#[derive(Debug)]
pub enum PlanState {
    Valid(Plan),
    /// Its `schema_version` isn't `1.0`.
    Unknown,
    /// Of `1.0`, but breaking the field reference: why, naming the field, never a value.
    Invalid(String),
    /// Valid, but its paths redacted by another `REDACTION_VERSION`.
    RedactionMismatch,
}

/// Rail Center's name for a plan, `sha256:` and 64 lowercase hex characters: compared, never
/// recomputed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentHash(String);

impl ContentHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn read(text: String) -> Option<Self> {
        let hex = text.strip_prefix("sha256:")?;
        let is_hex = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
        (hex.len() == 64 && hex.bytes().all(is_hex)).then_some(Self(text))
    }
}

/// A valid plan, what the proxy applies of it.
#[derive(Debug)]
pub struct Plan {
    pub window_seconds: u32,
    pub budget: Budget,
    /// Tried in this order.
    pub entries: Vec<Entry>,
    /// The catch-all's context keys; its measures are the three every record holds.
    pub catch_all: Vec<CatchAllKey>,
}

#[derive(Debug)]
pub struct Budget {
    pub max_groups_per_window: u32,
    /// Present: the order of calls is recorded too, up to this many steps.
    pub max_steps_per_window: Option<u32>,
    pub on_max_groups: OnMaxGroups,
    pub max_unsent_windows: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnMaxGroups {
    CoarsenToCatchAll,
}

#[derive(Debug)]
pub struct Entry {
    pub id: String,
    /// Read as written; checked when compiled, so one that fails refuses its entry, not the plan.
    pub conditions: Vec<Condition>,
    pub context_keys: Vec<ContextKey>,
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

/// Reads a plan Rail Center served to `agent`. `Err` for one that isn't a plan for it.
pub fn parse_plan(bytes: &[u8], agent: &Agent) -> Result<PlanReading, Refused> {
    let raw: Box<RawValue> = serde_json::from_slice(bytes).map_err(|_| Refused::NotJson)?;
    let fields: Fields = serde_json::from_str(raw.get()).map_err(|_| Refused::NoContentHash)?;
    let content_hash = peek::<String>(&fields, "content_hash")
        .and_then(ContentHash::read)
        .ok_or(Refused::NoContentHash)?;
    let named = peek::<Value>(&fields, "agent").unwrap_or_default();
    if named["host_id"] != agent.host_id.as_str()
        || named["sandbox_name"] != agent.sandbox_name.as_str()
    {
        return Err(Refused::OtherAgent);
    }
    let plan_version = peek::<u64>(&fields, "plan_version").filter(|version| *version >= 1);
    let state = if peek::<String>(&fields, "schema_version").as_deref() != Some(SCHEMA_VERSION) {
        PlanState::Unknown
    } else {
        match read_plan(&fields) {
            Err(why) => PlanState::Invalid(why),
            // Compared last, so a mismatch only describes a plan otherwise applied.
            Ok((_, redaction_version)) if redaction_version != REDACTION_VERSION => {
                PlanState::RedactionMismatch
            }
            Ok((plan, _)) => PlanState::Valid(plan),
        }
    };
    Ok(PlanReading {
        content_hash,
        plan_version,
        state,
    })
}

// Raw, so what serde_json can't hold fails a condition, not the plan.
type Fields = BTreeMap<String, Box<RawValue>>;

fn peek<T: DeserializeOwned>(fields: &Fields, name: &str) -> Option<T> {
    serde_json::from_str(fields.get(name)?.get()).ok()
}

// The plan and its `redaction_version`.
fn read_plan(fields: &Fields) -> Result<(Plan, u64), String> {
    if let Some(name) = fields.keys().find(|name| !FIELDS.contains(&name.as_str())) {
        return Err(format!("{name}: not a field of a plan"));
    }
    let generated_at: String = read_field(fields, "generated_at")?;
    if !is_utc_time(&generated_at) {
        return Err("generated_at: not an RFC 3339 time in UTC, with a Z".to_owned());
    }
    let agent: WireAgent = read_field(fields, "agent")?;
    if agent.agent_key.is_some_and(|key| key.is_empty()) {
        return Err("agent.agent_key: empty".to_owned());
    }
    let plan_version: u64 = read_field(fields, "plan_version")?;
    at_least_one(plan_version, "plan_version")?;
    let redaction_version: u64 = read_field(fields, "redaction_version")?;
    at_least_one(redaction_version, "redaction_version")?;
    let window_seconds: u32 = read_field(fields, "window_seconds")?;
    if !WINDOW_SECONDS.contains(&window_seconds) {
        return Err(format!(
            "window_seconds: outside {} to {}",
            WINDOW_SECONDS.start(),
            WINDOW_SECONDS.end()
        ));
    }
    let budget = read_budget(read_field(fields, "budget")?)?;
    let entries: Vec<Box<RawValue>> = read_field(fields, "entries")?;
    let entries = entries
        .iter()
        .enumerate()
        .map(|(index, raw)| read_entry(&format!("entries[{index}]"), raw))
        .collect::<Result<Vec<_>, _>>()?;
    let mut ids = HashSet::new();
    if let Some(index) = entries
        .iter()
        .position(|entry| !ids.insert(entry.id.as_str()))
    {
        return Err(format!("entries[{index}].id: an earlier entry's"));
    }
    let catch_all = read_catch_all(read_field(fields, "catch_all")?)?;
    let plan = Plan {
        window_seconds,
        budget,
        entries,
        catch_all,
    };
    Ok((plan, redaction_version))
}

fn read_field<T: DeserializeOwned>(fields: &Fields, name: &str) -> Result<T, String> {
    let raw = fields.get(name).ok_or_else(|| format!("{name}: missing"))?;
    read_raw(raw, name)
}

fn read_raw<T: DeserializeOwned>(raw: &RawValue, at: &str) -> Result<T, String> {
    serde_json::from_str(raw.get()).map_err(|error| format!("{at}: {}", describe(&error)))
}

// serde's message names a field, but may quote a value, which a reason never holds.
fn describe(error: &serde_json::Error) -> String {
    let message = error.to_string();
    let message = message.split(" at line ").next().unwrap_or_default();
    let names_a_field = ["unknown field", "missing field", "duplicate field"]
        .iter()
        .any(|kind| message.starts_with(kind));
    if names_a_field {
        message.split(',').next().unwrap_or_default().to_owned()
    } else {
        "a value of the wrong type or out of range".to_owned()
    }
}

fn at_least_one(value: u64, at: &str) -> Result<(), String> {
    if value == 0 {
        Err(format!("{at}: below 1"))
    } else {
        Ok(())
    }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireAgent {
    #[serde(rename = "host_id")]
    _host_id: String,
    #[serde(rename = "sandbox_name")]
    _sandbox_name: String,
    #[serde(default, deserialize_with = "present")]
    agent_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireBudget {
    max_groups_per_window: u32,
    #[serde(default, deserialize_with = "present")]
    max_steps_per_window: Option<u32>,
    on_max_groups: OnMaxGroups,
    max_unsent_windows: u32,
}

fn read_budget(budget: WireBudget) -> Result<Budget, String> {
    let groups = budget.max_groups_per_window;
    if !(1..=MAX_GROUPS_PER_WINDOW).contains(&groups) {
        return Err(format!(
            "budget.max_groups_per_window: outside 1 to {MAX_GROUPS_PER_WINDOW}"
        ));
    }
    if let Some(steps) = budget.max_steps_per_window
        && !(groups..=MAX_STEPS_PER_WINDOW).contains(&steps)
    {
        return Err(format!(
            "budget.max_steps_per_window: outside max_groups_per_window to {MAX_STEPS_PER_WINDOW}"
        ));
    }
    if budget.max_unsent_windows > MAX_UNSENT_WINDOWS {
        return Err(format!(
            "budget.max_unsent_windows: above {MAX_UNSENT_WINDOWS}"
        ));
    }
    Ok(Budget {
        max_groups_per_window: groups,
        max_steps_per_window: budget.max_steps_per_window,
        on_max_groups: budget.on_max_groups,
        max_unsent_windows: budget.max_unsent_windows,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireEntry {
    id: String,
    #[serde(default, deserialize_with = "present")]
    endpoint_id: Option<String>,
    #[serde(rename = "match")]
    conditions: Vec<Box<RawValue>>,
    record: WireRecord,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRecord {
    context_keys: Vec<WireKey>,
    measure: Vec<WireMeasure>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireKey {
    name: String,
    #[serde(default, deserialize_with = "present")]
    path: Option<Vec<Value>>,
    #[serde(default, deserialize_with = "present")]
    field_type: Option<String>,
    #[serde(default, deserialize_with = "present")]
    segments: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireMeasure {
    name: String,
    #[serde(rename = "type", default, deserialize_with = "present")]
    combine: Option<Combine>,
    #[serde(default, deserialize_with = "present")]
    function: Option<Function>,
    #[serde(default, deserialize_with = "present")]
    paths: Option<Vec<Vec<Value>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCatchAll {
    id: String,
    record: WireRecord,
}

// An optional field is left out, never `null`.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

fn read_entry(at: &str, raw: &RawValue) -> Result<Entry, String> {
    let entry: WireEntry = read_raw(raw, at)?;
    if entry.id.is_empty() || entry.id == CATCH_ALL {
        return Err(format!("{at}.id: empty or {CATCH_ALL}"));
    }
    if let Some(endpoint_id) = &entry.endpoint_id
        && !(1..=MAX_ENDPOINT_ID_CHARS).contains(&endpoint_id.chars().count())
    {
        return Err(format!(
            "{at}.endpoint_id: not 1 to {MAX_ENDPOINT_ID_CHARS} characters"
        ));
    }
    if entry.conditions.is_empty() {
        return Err(format!("{at}.match: empty"));
    }
    // A raw value holds no whitespace around it.
    if let Some(index) = entry
        .conditions
        .iter()
        .position(|raw| !raw.get().starts_with('{'))
    {
        return Err(format!("{at}.match[{index}]: not an object"));
    }
    let conditions = entry
        .conditions
        .iter()
        .map(|raw| read_condition(raw, Spelling::Plan))
        .collect();
    let record = &entry.record;
    unique_names(
        record.context_keys.iter().map(|key| &key.name),
        &format!("{at}.record.context_keys"),
    )?;
    let context_keys = record
        .context_keys
        .iter()
        .enumerate()
        .map(|(index, key)| read_context_key(&format!("{at}.record.context_keys[{index}]"), key))
        .collect::<Result<_, _>>()?;
    let measures = read_measures(&format!("{at}.record.measure"), &record.measure, true)?;
    Ok(Entry {
        id: entry.id,
        conditions,
        context_keys,
        measures,
    })
}

fn read_context_key(at: &str, key: &WireKey) -> Result<ContextKey, String> {
    match key.name.as_str() {
        "method" => {
            only_name(
                at,
                key.path.is_none() && key.field_type.is_none() && key.segments.is_none(),
            )?;
            Ok(ContextKey::Method)
        }
        "host" | "path_prefix" | "operation" => Err(format!("{at}: the catch-all's alone")),
        name => {
            if !is_name(name) {
                return Err(format!("{at}.name: {NOT_A_NAME}"));
            }
            if key.segments.is_some() {
                return Err(format!("{at}.segments: only on path_prefix"));
            }
            let path = key
                .path
                .as_deref()
                .ok_or_else(|| format!("{at}.path: missing"))?;
            let field_type = key
                .field_type
                .clone()
                .ok_or_else(|| format!("{at}.field_type: missing"))?;
            if !is_name(&field_type) {
                return Err(format!("{at}.field_type: {NOT_A_NAME}"));
            }
            Ok(ContextKey::Named {
                name: name.to_owned(),
                path: read_record_path(&format!("{at}.path"), path)?,
                field_type,
            })
        }
    }
}

// An entry's measures, or the catch-all's, which holds the three alone.
fn read_measures(at: &str, wire: &[WireMeasure], entry: bool) -> Result<Vec<Measure>, String> {
    unique_names(wire.iter().map(|measure| &measure.name), at)?;
    let measures = wire
        .iter()
        .enumerate()
        .map(|(index, measure)| read_measure(&format!("{at}[{index}]"), measure, entry))
        .collect::<Result<Vec<_>, _>>()?;
    for (built_in, name) in [
        (Measure::CallCount, "call_count"),
        (Measure::ErrorCount, "error_count"),
        (Measure::RequestBytes, "request_bytes"),
    ] {
        if !measures.contains(&built_in) {
            return Err(format!("{at}: no {name}"));
        }
    }
    Ok(measures)
}

fn read_measure(at: &str, measure: &WireMeasure, entry: bool) -> Result<Measure, String> {
    let parts = (measure.combine, measure.function, measure.paths.as_deref());
    match measure.name.as_str() {
        "call_count" | "error_count" => {
            only_name(at, parts == (None, None, None))?;
            Ok(match measure.name.as_str() {
                "call_count" => Measure::CallCount,
                _ => Measure::ErrorCount,
            })
        }
        "request_bytes" => {
            let whole_body = [vec![json!("request"), json!("body")]];
            match parts {
                (Some(Combine::Sum), Some(Function::Size), Some(paths)) if paths == whole_body => {
                    Ok(Measure::RequestBytes)
                }
                _ => Err(format!(
                    "{at}: not a sum of size over [[\"request\", \"body\"]]"
                )),
            }
        }
        _ if !entry => Err(format!(
            "{at}: the catch-all holds only the built-in measures"
        )),
        name => {
            if !is_name(name) {
                return Err(format!("{at}.name: {NOT_A_NAME}"));
            }
            let (Some(combine), Some(function), Some(paths)) = parts else {
                return Err(format!("{at}: missing its type, function or paths"));
            };
            if paths.is_empty() {
                return Err(format!("{at}.paths: empty"));
            }
            let paths = paths
                .iter()
                .enumerate()
                .map(|(index, path)| read_record_path(&format!("{at}.paths[{index}]"), path))
                .collect::<Result<_, _>>()?;
            Ok(Measure::Named {
                name: name.to_owned(),
                combine,
                function,
                paths,
            })
        }
    }
}

fn read_catch_all(catch_all: WireCatchAll) -> Result<Vec<CatchAllKey>, String> {
    if catch_all.id != CATCH_ALL {
        return Err(format!("catch_all.id: not {CATCH_ALL}"));
    }
    let at = "catch_all.record.context_keys";
    let wire = &catch_all.record.context_keys;
    unique_names(wire.iter().map(|key| &key.name), at)?;
    let keys = wire
        .iter()
        .enumerate()
        .map(|(index, key)| {
            let at = format!("{at}[{index}]");
            let named_alone = key.path.is_none() && key.field_type.is_none();
            match (key.name.as_str(), key.segments) {
                ("path_prefix", Some(segments)) if named_alone && segments >= 1 => {
                    Ok(CatchAllKey::PathPrefix { segments })
                }
                ("path_prefix", _) => Err(format!("{at}: not a name and segments from 1")),
                (name, segments) => {
                    let key = match name {
                        "host" => CatchAllKey::Host,
                        "method" => CatchAllKey::Method,
                        "operation" => CatchAllKey::Operation,
                        _ => return Err(format!("{at}: not a built-in key")),
                    };
                    only_name(&at, named_alone && segments.is_none())?;
                    Ok(key)
                }
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !keys.contains(&CatchAllKey::Host) {
        return Err(format!("{at}: no host"));
    }
    read_measures("catch_all.record.measure", &catch_all.record.measure, false)?;
    Ok(keys)
}

fn only_name(at: &str, holds_only_its_name: bool) -> Result<(), String> {
    if holds_only_its_name {
        Ok(())
    } else {
        Err(format!("{at}: a built-in holds only its name"))
    }
}

// A set by name.
fn unique_names<'a>(mut names: impl Iterator<Item = &'a String>, at: &str) -> Result<(), String> {
    let mut seen = HashSet::new();
    match names.position(|name| !seen.insert(name)) {
        Some(index) => Err(format!("{at}[{index}].name: an earlier one's")),
        None => Ok(()),
    }
}

const NOT_A_NAME: &str = "not lowercase letters, digits and _, from a letter, up to 32";

fn is_name(text: &str) -> bool {
    let mut bytes = text.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && text.len() <= 32
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

// A context key's or a measure's: into the body, the query or the redacted path's segments.
fn read_record_path(at: &str, steps: &[Value]) -> Result<Path, String> {
    let path = Path::from_steps(steps)
        .filter(|path| ConditionUse::PlanRecord.reads(path))
        .ok_or_else(|| {
            format!("{at}: not a path into the body, the query or a redacted path segment")
        })?;
    if let Path::Request(RequestPath::Body(body)) = &path
        && body
            .0
            .iter()
            .any(|step| matches!(step, BodyStep::Key(key) if key.is_empty()))
    {
        return Err(format!("{at}: an empty key"));
    }
    Ok(path)
}
