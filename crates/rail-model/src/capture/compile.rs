//! A capture plan compiled for matching calls: each entry's conditions compiled once, an entry
//! whose condition can't be evaluated refused, and the entries a call to a host is tried against
//! indexed by that host.

use std::collections::{BTreeSet, HashMap};

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
    /// For each host an entry's conditions imply, as written, the indexes of the entries a call to
    /// it is tried against, ending at the first refused one.
    walks: HashMap<String, Vec<usize>>,
    /// The walk of a call to any other host: the entries implying no host.
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

/// Why an entry takes no call: a condition in its `match` can't be evaluated. A call whose walk
/// reaches it stops there, for the catch-all, so a later, broader entry never takes a call meant
/// for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryRefusal {
    /// The first such condition's, in its `match`.
    pub position: usize,
    pub refusal: Refusal,
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

    /// The entries no call can reach: the refused ones, and those a refused entry before them in
    /// every walk they're in hides.
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
    let entries: Vec<CompiledEntry> = plan.entries.iter().map(compile_entry).collect();
    let hosts: Vec<Option<&str>> = plan.entries.iter().map(implied_host).collect();
    // A call to `host` is tried against the entries implying it or none, cut at the first refused.
    let walk = |host: Option<&str>| {
        let mut walk = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            if hosts[index].is_none() || hosts[index] == host {
                walk.push(index);
                if entry.refusal.is_some() {
                    break;
                }
            }
        }
        walk
    };
    let named: BTreeSet<&str> = hosts.iter().flatten().copied().collect();
    let walks: HashMap<String, Vec<usize>> = named
        .into_iter()
        .map(|host| (host.to_string(), walk(Some(host))))
        .collect();
    let other_hosts = walk(None);
    let reachable: BTreeSet<usize> = walks
        .values()
        .chain([&other_hosts])
        .flatten()
        .copied()
        .filter(|&index| entries[index].refusal.is_none())
        .collect();
    CompiledPlan {
        entries_skipped: entries.len() - reachable.len(),
        entries,
        walks,
        other_hosts,
        plan,
    }
}

fn compile_entry(entry: &Entry) -> CompiledEntry {
    let mut refusal = None;
    let mut request_line = Vec::new();
    let mut body = Vec::new();
    for (position, condition) in entry.conditions.iter().enumerate() {
        let compiled = compile_condition(condition, ConditionUse::CapturePlan);
        if refusal.is_none()
            && let Some(why) = compiled.refusal()
        {
            refusal = Some(EntryRefusal {
                position,
                refusal: why,
            });
        }
        if reads_the_body(condition) {
            body.push(compiled);
        } else {
            request_line.push(compiled);
        }
    }
    CompiledEntry {
        id: entry.id.clone(),
        refusal,
        request_line,
        body,
    }
}

fn reads_the_body(condition: &Condition) -> bool {
    let Condition::Valid(predicate) = condition else {
        return false;
    };
    matches!(predicate.path(), Path::Request(path) if path.reads_the_body())
}

// The host every call the entry takes is to: one its "and" requires, by an `eq` on a string.
fn implied_host(entry: &Entry) -> Option<&str> {
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
