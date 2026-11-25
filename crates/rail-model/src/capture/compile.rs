//! A capture plan compiled for matching calls: each entry's conditions compiled once, an entry
//! whose condition can't be evaluated refused, and the entries a call to a host is tried against
//! indexed by that host.

use std::collections::{HashMap, HashSet};

use super::plan::{Entry, Plan};
use crate::condition::{
    CompiledCondition, ConditionUse, LocationSegment, Refusal, compile_condition,
};

/// A plan compiled for matching: built once per plan, then used for every call.
#[derive(Debug)]
pub struct CompiledPlan {
    plan: Plan,
    /// In plan order, one per entry.
    entries: Vec<CompiledEntry>,
    /// For each host an entry's conditions imply, as written, the indexes of those entries.
    hosts: HashMap<String, Vec<usize>>,
    /// The indexes of the entries implying no host, tried for every host.
    any_host: Vec<usize>,
    entries_skipped: usize,
}

/// An entry compiled for matching.
#[derive(Debug)]
pub struct CompiledEntry {
    id: String,
    refusal: Option<EntryRefusal>,
    /// The host its conditions require, if they do.
    host: Option<String>,
    /// Its "and", the request line's before the body's, each in plan order: the body is read only
    /// for an entry the request line fits.
    conditions: Vec<CompiledCondition>,
}

/// Why an entry takes no call: a condition in its `match` can't be evaluated. A call whose walk
/// reaches it stops there, for the catch-all, so a later, broader entry never takes a call meant
/// for it.
#[derive(Clone, Debug, PartialEq)]
pub struct EntryRefusal {
    /// Where the first such condition is, from its `match`: `[3]` for its fourth.
    pub location: Vec<LocationSegment>,
    pub refusal: Refusal,
}

impl CompiledEntry {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Why it takes no call, if it can't.
    pub fn refusal(&self) -> Option<&EntryRefusal> {
        self.refusal.as_ref()
    }

    pub(crate) fn conditions(&self) -> &[CompiledCondition] {
        &self.conditions
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

    /// The entry at `index`, compiled.
    #[cfg(feature = "conformance")]
    pub(crate) fn entry(&self, index: usize) -> Option<&CompiledEntry> {
        self.entries.get(index)
    }

    /// The entries a call to `host` is tried against, in plan order, the last refused if one is.
    pub(crate) fn entries_for<'a>(
        &'a self,
        host: &str,
    ) -> impl Iterator<Item = (&'a Entry, &'a CompiledEntry)> {
        let own = self.hosts.get(host).map_or(&[][..], Vec::as_slice);
        let (mut own, mut any) = (own.iter().peekable(), self.any_host.iter().peekable());
        // The two lists merged in plan order, each index in one of them only.
        let merged = std::iter::from_fn(move || match (own.peek(), any.peek()) {
            (Some(a), b) if b.is_none_or(|b| a < b) => own.next(),
            _ => any.next(),
        });
        let mut ended = false;
        merged
            .map(|&index| (&self.plan.entries[index], &self.entries[index]))
            .take_while(move |(_, entry)| !std::mem::replace(&mut ended, entry.refusal.is_some()))
    }
}

/// Compiles every entry of `plan`, which [`parse_plan`](super::parse_plan) has read.
pub fn compile_plan(plan: Plan) -> CompiledPlan {
    let entries: Vec<CompiledEntry> = plan.entries.iter().map(compile_entry).collect();
    let mut hosts: HashMap<String, Vec<usize>> = HashMap::new();
    let mut any_host = Vec::new();
    // An entry is reached unless a refused one before it, implying its host or none, ends its walk.
    let mut refused_hosts = HashSet::new();
    let mut refused_any = false;
    let mut entries_skipped = 0;
    for (index, entry) in entries.iter().enumerate() {
        let hidden = refused_any
            || entry
                .host
                .as_ref()
                .is_some_and(|host| refused_hosts.contains(host));
        let refused = entry.refusal.is_some();
        if hidden || refused {
            entries_skipped += 1;
        }
        match &entry.host {
            Some(host) => {
                hosts.entry(host.clone()).or_default().push(index);
                if refused {
                    refused_hosts.insert(host.clone());
                }
            }
            None => {
                any_host.push(index);
                refused_any |= refused;
            }
        }
    }
    CompiledPlan {
        plan,
        entries,
        hosts,
        any_host,
        entries_skipped,
    }
}

fn compile_entry(entry: &Entry) -> CompiledEntry {
    let mut refusal = None;
    let mut host = None;
    let mut conditions = Vec::with_capacity(entry.conditions.len());
    for (position, condition) in entry.conditions.iter().enumerate() {
        let compiled = compile_condition(condition, ConditionUse::CapturePlan);
        if refusal.is_none()
            && let Some(why) = compiled.refusal()
        {
            refusal = Some(EntryRefusal {
                location: vec![LocationSegment::Index(position)],
                refusal: why,
            });
        }
        // The first, as every condition of the entry's "and" holds for a call it takes.
        if host.is_none() {
            host = compiled.implied_host().map(str::to_string);
        }
        conditions.push(compiled);
    }
    // Stable, so a group keeps plan order; the refusal's location is already taken from it.
    conditions.sort_by_key(CompiledCondition::reads_the_body);
    CompiledEntry {
        id: entry.id.clone(),
        refusal,
        host,
        conditions,
    }
}
