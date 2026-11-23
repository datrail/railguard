//! A capture plan compiled for matching calls: each entry's conditions compiled once, the entries
//! a plan can't apply refused, and the entries a call to a host can reach indexed by that host.

use std::collections::HashMap;
use std::iter::Peekable;

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
    /// For each host an entry's `["request", "host"]` `eq` names, as written, its entries' indexes.
    hosts: HashMap<String, Vec<usize>>,
    /// The refused entries naming no host: every call's walk stops at them.
    barriers: Vec<usize>,
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

    /// The entries a call to `host` is tried against, in plan order: its host's, and the barriers.
    pub fn entries_for<'a>(&'a self, host: &str) -> impl Iterator<Item = &'a CompiledEntry> {
        let own = self.hosts.get(host).map_or(&[][..], Vec::as_slice);
        Merged {
            own: own.iter().peekable(),
            barriers: self.barriers.iter().peekable(),
        }
        .map(|index| &self.entries[index])
    }
}

/// Compiles every entry of `plan`, which [`parse_plan`](super::parse_plan) has read.
pub fn compile_plan(plan: Plan) -> CompiledPlan {
    let entries: Vec<CompiledEntry> = plan.entries.iter().map(compile_entry).collect();
    let mut hosts: HashMap<String, Vec<usize>> = HashMap::new();
    let mut barriers = Vec::new();
    for (index, entry) in plan.entries.iter().enumerate() {
        match read_host(entry) {
            Some(host) => hosts.entry(host.to_string()).or_default().push(index),
            None if entries[index].refusal.is_some() => barriers.push(index),
            // Never: an entry naming no host doesn't pin one endpoint, so it's refused.
            None => {}
        }
    }
    let entries_skipped = count_skipped(&entries, &hosts, &barriers);
    CompiledPlan {
        plan,
        entries,
        hosts,
        barriers,
        entries_skipped,
    }
}

fn compile_entry(entry: &Entry) -> CompiledEntry {
    let mut refusal = None;
    let mut request_line = Vec::new();
    let mut body = Vec::new();
    for (position, condition) in entry.conditions.iter().enumerate() {
        let compiled = compile_condition(condition, ConditionUse::PlanMatch);
        if refusal.is_none()
            && let Some(why) = refuse(condition, &compiled)
        {
            refusal = Some(EntryRefusal::Condition {
                position,
                refusal: why,
            });
        }
        match condition {
            Condition::Valid(predicate) if reads_the_body(predicate.path()) => body.push(compiled),
            _ => request_line.push(compiled),
        }
    }
    if refusal.is_none() && !pins_one_endpoint(entry) {
        refusal = Some(EntryRefusal::NotOneEndpoint);
    }
    CompiledEntry {
        id: entry.id.clone(),
        refusal,
        request_line,
        body,
    }
}

// The checker's refusal, or the plan's own: an `in` with no value holds for nothing, so it can only
// be a mistake.
fn refuse(condition: &Condition, compiled: &CompiledCondition) -> Option<Refusal> {
    if let Some(refusal) = compiled.refusal() {
        return Some(refusal);
    }
    match condition {
        Condition::Valid(Predicate::In { value, .. }) if value.is_empty() => Some(Refusal::Invalid),
        _ => None,
    }
}

fn reads_the_body(path: &Path) -> bool {
    matches!(
        path,
        Path::Request(RequestPath::Body(_) | RequestPath::McpMethod | RequestPath::McpTool)
    )
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

// An MCP tool: `eq` on the host, the redacted path and the tool; an HTTP operation: `eq` on the host
// and the method, and `eq` or `matches` on the path. Further conditions only narrow it.
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
    pins(&RequestPath::Host, false) && (mcp_tool || http_operation)
}

// An entry no call reaches: refused, or after a refused entry on its host, or after a barrier.
fn count_skipped(
    entries: &[CompiledEntry],
    hosts: &HashMap<String, Vec<usize>>,
    barriers: &[usize],
) -> usize {
    let first_barrier = barriers.first().copied().unwrap_or(usize::MAX);
    let hidden_on_host = |indexes: &Vec<usize>| {
        let first_refused = indexes
            .iter()
            .copied()
            .find(|&index| entries[index].refusal.is_some())
            .unwrap_or(usize::MAX);
        indexes
            .iter()
            .filter(|&&index| entries[index].refusal.is_none())
            .filter(|&&index| index > first_refused.min(first_barrier))
            .count()
    };
    let refused = entries
        .iter()
        .filter(|entry| entry.refusal.is_some())
        .count();
    refused + hosts.values().map(hidden_on_host).sum::<usize>()
}

// Two lists of indexes, each in plan order, walked as one.
struct Merged<'a> {
    own: Peekable<std::slice::Iter<'a, usize>>,
    barriers: Peekable<std::slice::Iter<'a, usize>>,
}

impl Iterator for Merged<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let next = match (self.own.peek(), self.barriers.peek()) {
            (Some(own), Some(barrier)) if own < barrier => &mut self.own,
            (Some(_), None) => &mut self.own,
            _ => &mut self.barriers,
        };
        next.next().copied()
    }
}
