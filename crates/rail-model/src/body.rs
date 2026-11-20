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
    Unkept(Unkept),
    /// The reader wasn't sent it.
    NotSent,
    /// The reader was sent only part of it.
    CutShort,
}

/// The bytes every kept body and its tree share, so many large bodies at once can't exhaust memory.
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
struct Charge {
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
/// byte.
pub struct BodyBuffer {
    bytes: Vec<u8>,
    cap_bytes: usize,
    received_bytes: u64,
    charge: Charge,
    unkept: Option<Unkept>,
}

impl BodyBuffer {
    pub fn new(cap_bytes: usize, budget: &Arc<ByteBudget>) -> Self {
        Self {
            bytes: Vec::new(),
            cap_bytes,
            received_bytes: 0,
            charge: Charge::new(budget),
            unkept: None,
        }
    }

    /// Keeps `chunk`, unless the body would pass the cap or the budget: then every byte kept is
    /// dropped, and none after is kept.
    pub fn append(&mut self, chunk: &[u8]) {
        self.received_bytes += chunk.len() as u64;
        if self.unkept.is_some() {
            return;
        }
        let length_bytes = self.bytes.len() + chunk.len();
        if length_bytes > self.cap_bytes {
            return self.drop_bytes(Unkept::PastCap);
        }
        let capacity_bytes = self.bytes.capacity();
        if length_bytes > capacity_bytes {
            // Doubling, as a `Vec` does, but within the cap, and charged before it's taken.
            let wanted_bytes = length_bytes.max(capacity_bytes * 2).min(self.cap_bytes);
            if !self.charge.grow(wanted_bytes - capacity_bytes) {
                return self.drop_bytes(Unkept::PastBudget);
            }
            self.bytes.reserve_exact(wanted_bytes - self.bytes.len());
        }
        self.bytes.extend_from_slice(chunk);
    }

    /// Every byte appended, kept or not.
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
        self.charge.release();
    }
}

/// A body whole: immutable, to share as an `Arc`, its charge held until it's dropped. Each reading
/// is computed once, by whichever caller asks first.
pub struct RequestBody {
    bytes: Vec<u8>,
    charge: Charge,
    // With its own charge, on the same budget.
    tree: OnceLock<Result<(Tree, Charge), Unparsed>>,
    mcp: OnceLock<McpReading>,
}

impl RequestBody {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The body read as JSON: its top value, past one UTF-8 BOM.
    pub fn json(&self) -> Result<Node<'_>, Unparsed> {
        let tree = self.tree.get_or_init(|| {
            let mut charge = Charge::new(&self.charge.budget);
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
