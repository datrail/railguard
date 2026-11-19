//! A request body: kept as it arrives, within a cap and a budget it shares with every other, then
//! read once, by whichever caller asks first, as a tree and as an MCP message.

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
    /// Not one JSON value, nested past 256 levels, or past 4 GiB. A string's contents aren't
    /// checked until it's read.
    NotJson,
    /// Its tree would pass the byte budget.
    PastBudget,
}

/// Why a body isn't kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unkept {
    PastCap,
    PastBudget,
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
        let length = self.bytes.len() + chunk.len();
        if length > self.cap_bytes {
            return self.drop_bytes(Unkept::PastCap);
        }
        let capacity = self.bytes.capacity();
        if length > capacity {
            // Doubling, as a `Vec` does, but within the cap, and charged before it's taken.
            let wanted = length.max(capacity * 2).min(self.cap_bytes);
            if !self.charge.grow(wanted - capacity) {
                return self.drop_bytes(Unkept::PastBudget);
            }
            self.bytes.reserve_exact(wanted - self.bytes.len());
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

    /// The body's top value, past one UTF-8 BOM. `Content-Encoding` isn't decoded.
    pub fn root(&self) -> Result<Node<'_>, Unparsed> {
        let tree = self.tree.get_or_init(|| {
            let mut charge = Charge::new(&self.charge.budget);
            let tree = Tree::parse(&self.bytes, &mut |bytes| charge.grow(bytes))?;
            Ok((tree, charge))
        });
        match tree {
            Ok((tree, _)) => Ok(tree.root(&self.bytes)),
            Err(why) => Err(*why),
        }
    }

    /// What the body names as an MCP message.
    pub fn mcp(&self) -> &McpReading {
        self.mcp
            .get_or_init(|| McpReading::of(self.bytes.is_empty(), self.root()))
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

use std::borrow::Cow;

use crate::endpoint::{Endpoint, Resolution, resolve_endpoint};
use serde::de::{DeserializeSeed, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

// Past this, a body names nothing.
const MAX_NESTING: usize = 256;

/// What `body`, as sent to `path`, asks for. One that isn't a JSON object in UTF-8, or nests past
/// 256 levels, names no endpoint this reader can key; a `method` or `name` of another type reads as
/// absent. `Content-Encoding` isn't decoded.
pub fn read_endpoint(path: &str, body: &[u8]) -> Resolution {
    // Read past one UTF-8 BOM, which a JSON reader may accept.
    let body = body.strip_prefix("\u{feff}".as_bytes()).unwrap_or(body);
    if nests_past(body, MAX_NESTING) {
        return Resolution::Judged(Endpoint::Unrecognised);
    }
    let Some(message) = members(body) else {
        return Resolution::Judged(Endpoint::Unrecognised);
    };
    let method = last(&message, "method").and_then(text);
    let name = last(&message, "params")
        .and_then(|params| members(params.get().as_bytes()))
        .and_then(|params| last(&params, "name"))
        .and_then(text);
    resolve_endpoint(path, method.as_deref().unwrap_or(""), name.as_deref())
}

// An object's keys and values, in order, each value only scanned.
type Members<'a> = Vec<(Cow<'a, [u8]>, &'a RawValue)>;

fn members(json: &[u8]) -> Option<Members<'_>> {
    let mut deserializer = serde_json::Deserializer::from_slice(json);
    let members = deserializer.deserialize_map(Object).ok()?;
    deserializer.end().ok()?;
    Some(members)
}

// A key repeated keeps its last value.
fn last<'a>(members: &Members<'a>, key: &str) -> Option<&'a RawValue> {
    let key = key.as_bytes();
    let (_, value) = members.iter().rev().find(|(name, _)| &**name == key)?;
    Some(value)
}

// A string's text; `None` for any other value.
fn text(value: &RawValue) -> Option<String> {
    serde_json::from_str(value.get()).ok()
}

// Brackets outside strings, counted before parsing, so a deep body can't exhaust the parser.
fn nests_past(body: &[u8], limit: usize) -> bool {
    let mut depth = 0_usize;
    let (mut in_string, mut escaped) = (false, false);
    for &byte in body {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

// Visits an object, keeping each key and its value as sent.
struct Object;

impl<'de> Visitor<'de> for Object {
    type Value = Members<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut members = Vec::new();
        while let Some(key) = map.next_key_seed(KeyBytes)? {
            members.push((key, map.next_value()?));
        }
        Ok(members)
    }
}

// A key as bytes, so one holding a lone surrogate escape fails nothing.
struct KeyBytes;

impl<'de> DeserializeSeed<'de> for KeyBytes {
    type Value = Cow<'de, [u8]>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_bytes(self)
    }
}

impl<'de> Visitor<'de> for KeyBytes {
    type Value = Cow<'de, [u8]>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a key")
    }

    fn visit_borrowed_bytes<E: serde::de::Error>(self, key: &'de [u8]) -> Result<Self::Value, E> {
        Ok(Cow::Borrowed(key))
    }

    fn visit_bytes<E: serde::de::Error>(self, key: &[u8]) -> Result<Self::Value, E> {
        Ok(Cow::Owned(key.to_vec()))
    }

    // Not called by serde_json today; kept so a key handed over as text still reads.
    fn visit_borrowed_str<E: serde::de::Error>(self, key: &'de str) -> Result<Self::Value, E> {
        Ok(Cow::Borrowed(key.as_bytes()))
    }

    fn visit_str<E: serde::de::Error>(self, key: &str) -> Result<Self::Value, E> {
        Ok(Cow::Owned(key.as_bytes().to_vec()))
    }
}
