//! A capture plan compiled for matching calls: each entry's conditions compiled once, the entries
//! a plan can't apply refused, and the entries a call to a host can reach indexed by that host.

use std::collections::{HashMap, HashSet};

use super::plan::{Entry, Plan};
use crate::condition::{
    CompiledCondition, Condition, ConditionUse, Path, Predicate, Refusal, RequestPath, Scalar,
    compile_condition,
};

/// A plan compiled for matching: built once per plan, then used for every call.
#[derive(Debug)]
pub struct CompiledPlan {
    plan: Plan,
    /// In plan order, one per entry.
    entries: Vec<CompiledEntry>,
    /// For each host an entry's `["request", "host"]` `eq` names, as written, the indexes of the
    /// entries a call to it is tried against, ending at the first refused one it reaches.
    walks: HashMap<String, Vec<usize>>,
    /// The walk of a call to any other host: the first refused entry naming no host, if any.
    other_hosts: Vec<usize>,
    entries_skipped: usize,
}

/// An entry compiled for matching, its conditions split so the request line is read first.
#[derive(Debug)]
pub struct CompiledEntry {
    id: String,
    refusal: Option<EntryRefusal>,
    #[expect(dead_code, reason = "read by matching a call, still to come")]
    request_line: Vec<CompiledCondition>,
    #[expect(dead_code, reason = "read by matching a call, still to come")]
    body: Vec<CompiledCondition>,
}

/// Why an entry takes no call. A call whose walk reaches it stops there, for the catch-all, so a
/// later, broader entry never takes a call meant for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryRefusal {
    /// The condition at this position in its `match` can't be evaluated for a plan entry.
    Condition { position: usize, refusal: Refusal },
    /// Its conditions don't pin one endpoint, so its records could add two endpoints' calls.
    NotOneEndpoint,
}

impl CompiledEntry {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Why it takes no call, if it can't.
    pub fn refusal(&self) -> Option<EntryRefusal> {
        self.refusal
    }
}

impl CompiledPlan {
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// The refused entries, in plan order, each once per plan.
    pub fn refused(&self) -> impl Iterator<Item = &CompiledEntry> {
        self.entries.iter().filter(|entry| entry.refusal.is_some())
    }

    /// The entries no call can reach: the refused ones, and those a refused entry before them
    /// hides, on their host or, naming none, on every host.
    pub fn entries_skipped(&self) -> usize {
        self.entries_skipped
    }

    /// The entries a call to `host` is tried against, in plan order, the last refused if one is.
    #[expect(dead_code, reason = "read by matching a call, still to come")]
    pub(crate) fn entries_for<'a>(&'a self, host: &str) -> impl Iterator<Item = &'a CompiledEntry> {
        let walk = self.walks.get(host).unwrap_or(&self.other_hosts);
        walk.iter().map(|&index| &self.entries[index])
    }
}

/// Compiles every entry of `plan`, which [`parse_plan`](super::parse_plan) has read.
pub fn compile_plan(plan: Plan) -> CompiledPlan {
    let mut entries = Vec::with_capacity(plan.entries.len());
    let mut walks: HashMap<String, Vec<usize>> = HashMap::new();
    // The hosts whose walk already ends at a refused entry of their own.
    let mut ended = HashSet::new();
    let mut barrier = None;
    let mut reachable = 0;
    for (index, entry) in plan.entries.iter().enumerate() {
        let host = read_host(entry);
        let compiled = compile_entry(entry, host.is_some());
        let refused = compiled.refusal.is_some();
        entries.push(compiled);
        match host {
            // Past a barrier, no call reaches it.
            _ if barrier.is_some() => {}
            Some(host) if !ended.contains(host) => {
                walks.entry(host.to_string()).or_default().push(index);
                if refused {
                    ended.insert(host);
                } else {
                    reachable += 1;
                }
            }
            Some(_) => {}
            // An entry naming no host doesn't pin one endpoint, so it's refused.
            None => barrier = Some(index),
        }
    }
    if let Some(barrier) = barrier {
        for (host, walk) in &mut walks {
            if !ended.contains(host.as_str()) {
                walk.push(barrier);
            }
        }
    }
    CompiledPlan {
        entries_skipped: entries.len() - reachable,
        entries,
        walks,
        other_hosts: barrier.into_iter().collect(),
        plan,
    }
}

fn compile_entry(entry: &Entry, names_a_host: bool) -> CompiledEntry {
    let mut refusal = None;
    let mut request_line = Vec::new();
    let mut body = Vec::new();
    for (position, condition) in entry.conditions.iter().enumerate() {
        let compiled = compile_condition(condition, ConditionUse::CapturePlan);
        if refusal.is_none()
            && let Some(why) = compiled.refusal()
        {
            refusal = Some(EntryRefusal::Condition {
                position,
                refusal: why,
            });
        }
        match condition {
            Condition::Valid(predicate) if matches!(predicate.path(), Path::Request(path) if path.reads_the_body()) => {
                body.push(compiled)
            }
            _ => request_line.push(compiled),
        }
    }
    if refusal.is_none() && !(names_a_host && pins_one_endpoint(entry)) {
        refusal = Some(EntryRefusal::NotOneEndpoint);
    }
    CompiledEntry {
        id: entry.id.clone(),
        refusal,
        request_line,
        body,
    }
}

// The first `["request", "host"]` `eq` on a string, which the plan's host totals count toward.
fn read_host(entry: &Entry) -> Option<&str> {
    entry
        .conditions
        .iter()
        .find_map(|condition| match condition {
            Condition::Valid(Predicate::Eq {
                path: Path::Request(RequestPath::Host),
                value: Scalar::String(host),
            }) => Some(host.as_str()),
            _ => None,
        })
}

// Beside the host: an MCP tool, `eq` on the redacted path and the tool; an HTTP operation, `eq` on
// the method, and `eq` or `matches` on the path. Further conditions only narrow it.
fn pins_one_endpoint(entry: &Entry) -> bool {
    let pins = |field: &RequestPath, glob: bool| {
        entry.conditions.iter().any(|condition| match condition {
            Condition::Valid(Predicate::Eq {
                path: Path::Request(path),
                value: Scalar::String(_),
            }) => path == field,
            Condition::Valid(Predicate::Matches {
                path: Path::Request(path),
                ..
            }) => glob && path == field,
            _ => false,
        })
    };
    let mcp_tool =
        pins(&RequestPath::RedactedPath(None), false) && pins(&RequestPath::McpTool, false);
    let http_operation = pins(&RequestPath::Method, false) && pins(&RequestPath::Path(None), true);
    mcp_tool || http_operation
}
