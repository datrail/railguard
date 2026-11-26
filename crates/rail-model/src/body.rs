//! A request body: kept as it arrives, within a cap and a budget it shares with every other, then
//! read once, by whichever caller asks first, as a tree of values and as an MCP message.

mod json;
pub mod mcp;
pub mod tree;

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use mcp::McpReading;
use tree::{Node, Tree};

/// Why a body isn't read as a tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unparsed {
    /// Not one value in the format read, nested past 256 levels, or past 4 GiB. A string's
    /// contents aren't checked until it's read.
    Malformed,
    /// Its tree would pass the byte budget.
    PastBudget,
}

/// Why a body isn't kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unkept {
    PastCap,
    PastBudget,
}

/// A call's body as a reader has it: kept whole, or unread and why.
#[derive(Clone, Debug)]
pub enum BodyRead {
    Whole(Arc<RequestBody>),
    Unread(Unread),
}

/// Why a call's body isn't read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unread {
    PastCap,
    PastBudget,
    /// The reader wasn't sent it.
    NotSent,
    /// The reader was sent only part of it.
    CutShort,
}

impl From<Unkept> for Unread {
    fn from(unkept: Unkept) -> Self {
        match unkept {
            Unkept::PastCap => Self::PastCap,
            Unkept::PastBudget => Self::PastBudget,
        }
    }
}

/// The bytes every body kept within it, its tree and the headers conditions read share, so many
/// large requests at once can't exhaust memory.
#[derive(Debug)]
pub struct ByteBudget {
    limit_bytes: usize,
    used_bytes: AtomicUsize,
}

impl ByteBudget {
    pub fn new(limit_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            limit_bytes,
            used_bytes: AtomicUsize::new(0),
        })
    }
}

// Bytes held against a budget, given back when dropped.
pub(crate) struct Charge {
    budget: Arc<ByteBudget>,
    bytes: usize,
}

impl Charge {
    fn new(budget: &Arc<ByteBudget>) -> Self {
        Self {
            budget: Arc::clone(budget),
            bytes: 0,
        }
    }

    // Takes `bytes` more, unless that would pass the budget.
    fn grow(&mut self, bytes: usize) -> bool {
        let limit = self.budget.limit_bytes;
        let taken = self
            .budget
            .used_bytes
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|&total| total <= limit)
            })
            .is_ok();
        if taken {
            self.bytes += bytes;
        }
        taken
    }

    fn release(&mut self) {
        self.budget
            .used_bytes
            .fetch_sub(self.bytes, Ordering::AcqRel);
        self.bytes = 0;
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.release();
    }
}

/// A body as it arrives: kept up to a cap, the memory it takes charged to a budget from its first
/// byte, but for a first body adopted past it, when it keeps those.
pub struct BodyBuffer {
    bytes: Vec<u8>,
    cap_bytes: usize,
    received_bytes: u64,
    budget: Arc<ByteBudget>,
    // `None` for a body adopted past the budget, kept outside it.
    charge: Option<Charge>,
    keep_past_budget: bool,
    unkept: Option<Unkept>,
}

impl BodyBuffer {
    pub fn new(cap_bytes: usize, budget: &Arc<ByteBudget>) -> Self {
        Self {
            bytes: Vec::new(),
            cap_bytes,
            received_bytes: 0,
            budget: Arc::clone(budget),
            charge: Some(Charge::new(budget)),
            keep_past_budget: false,
            unkept: None,
        }
    }

    /// From now, a first body adopted past the budget is kept outside it, its tree refused, for a
    /// host that sends it back: it holds it already.
    pub fn keep_past_budget(&mut self) {
        self.keep_past_budget = true;
    }

    /// Keeps `chunk`, unless the body would pass the cap or the budget: then every byte kept is
    /// dropped, and none after is kept. A body kept outside the budget is charged whole then.
    pub fn append(&mut self, chunk: &[u8]) {
        self.received_bytes += chunk.len() as u64;
        if self.unkept.is_some() || chunk.is_empty() {
            return;
        }
        let length_bytes = self.bytes.len() + chunk.len();
        if length_bytes > self.cap_bytes {
            return self.drop_bytes(Unkept::PastCap);
        }
        if self.charge.is_none() {
            let mut charge = Charge::new(&self.budget);
            if !charge.grow(self.bytes.capacity()) {
                return self.drop_bytes(Unkept::PastBudget);
            }
            self.charge = Some(charge);
        }
        let capacity_bytes = self.bytes.capacity();
        if length_bytes > capacity_bytes {
            // Doubling, as a `Vec` does, but within the cap, and charged before it's taken.
            let wanted_bytes = length_bytes.max(capacity_bytes * 2).min(self.cap_bytes);
            let grown = (self.charge.as_mut())
                .is_some_and(|charge| charge.grow(wanted_bytes - capacity_bytes));
            if !grown {
                return self.drop_bytes(Unkept::PastBudget);
            }
            self.bytes.reserve_exact(wanted_bytes - self.bytes.len());
        }
        self.bytes.extend_from_slice(chunk);
    }

    /// Keeps `body` as `append` would, without a copy if it's the first: its capacity charged.
    pub fn adopt(&mut self, body: Vec<u8>) {
        if self.received_bytes != 0 || self.bytes.capacity() != 0 || body.len() > self.cap_bytes {
            return self.append(&body);
        }
        self.received_bytes = body.len() as u64;
        let charged = (self.charge.as_mut()).is_some_and(|charge| charge.grow(body.capacity()));
        if !charged {
            if !self.keep_past_budget {
                return self.drop_bytes(Unkept::PastBudget);
            }
            self.charge = None;
        }
        self.bytes = body;
    }

    /// Whether every byte so far is kept: none is past the cap, nor past the budget but a first
    /// body adopted keeping it.
    pub fn is_kept(&self) -> bool {
        self.unkept.is_none()
    }

    /// Every byte appended or adopted, kept or not.
    pub fn received_bytes(&self) -> u64 {
        self.received_bytes
    }

    pub fn finish(self) -> Result<RequestBody, Unkept> {
        if let Some(unkept) = self.unkept {
            return Err(unkept);
        }
        Ok(RequestBody {
            bytes: self.bytes,
            charge: self.charge,
            tree: OnceLock::new(),
            mcp: OnceLock::new(),
        })
    }

    fn drop_bytes(&mut self, why: Unkept) {
        self.unkept = Some(why);
        self.bytes = Vec::new();
        if let Some(charge) = &mut self.charge {
            charge.release();
        }
    }
}

/// A body whole: immutable, to share as an `Arc`, its charge, if any, held until it's dropped. Each
/// reading is computed once, by whichever caller asks first.
pub struct RequestBody {
    bytes: Vec<u8>,
    // `None` for a body kept outside the budget.
    charge: Option<Charge>,
    // With its own charge, on the same budget.
    tree: OnceLock<Result<(Tree, Charge), Unparsed>>,
    mcp: OnceLock<McpReading>,
}

impl RequestBody {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The bytes, for a host to send on, its charge given back.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// The body read as JSON: its top value, past one UTF-8 BOM.
    pub fn json(&self) -> Result<Node<'_>, Unparsed> {
        let tree = self.tree.get_or_init(|| {
            let mut charge = self.new_charge().ok_or(Unparsed::PastBudget)?;
            let tree = json::parse(self.content(), &mut |bytes| charge.grow(bytes))?;
            Ok((tree, charge))
        });
        tree.as_ref()
            .map(|(tree, _)| tree.root(self.content()))
            .map_err(|&why| why)
    }

    /// What the body names as an MCP message.
    pub fn mcp(&self) -> &McpReading {
        self.mcp
            .get_or_init(|| McpReading::of(self.content(), self.json()))
    }

    // `bytes` charged to its budget, for what's read from its request; `None` past it, or outside it.
    pub(crate) fn charge_for(&self, bytes: usize) -> Option<Charge> {
        let mut charge = self.new_charge()?;
        charge.grow(bytes).then_some(charge)
    }

    fn new_charge(&self) -> Option<Charge> {
        self.charge
            .as_ref()
            .map(|charge| Charge::new(&charge.budget))
    }

    // What the readings parse: the bytes as sent, until `Content-Encoding` is decoded.
    fn content(&self) -> &[u8] {
        &self.bytes
    }
}

// Not the bytes, which can hold credentials.
impl fmt::Debug for RequestBody {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter
            .debug_struct("RequestBody")
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}
