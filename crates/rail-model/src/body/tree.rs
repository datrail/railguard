//! A body's content as a tree of values: positions into its bytes, each string decoded only when
//! read, by its format's decoder. Each format's parser builds it, so a value is found one way
//! whatever the format.
//!
//! The tree is one flat list of fixed-size entries, every key and value in document order, each
//! container's entry noting where its subtree ends. Not nested nodes: those take an allocation per
//! object and array and about twice the memory, which the byte budget could only estimate. So
//! parsing allocates little and its memory is charged exactly; finding a key is a scan either way,
//! but the `n`th item of an array takes `n` hops, not one.

use std::borrow::Cow;
use std::fmt;

use super::Unparsed;

// Past this, content isn't read.
const MAX_NESTING: usize = 256;

/// Every key and value in a body's content, in document order, each container followed by its
/// subtree.
#[derive(Debug)]
pub(crate) struct Tree {
    entries: Vec<Entry>,
    decode: Decode,
    conforms: Conforms,
}

/// A string's text from its raw text, `escaped` if that holds anything to decode; `None` for one
/// its format can't hold.
type Decode = fn(&[u8], bool) -> Option<Cow<'_, str>>;

/// Whether a key `Decode` refuses, from its raw text, reads as no key sought to every reader: of
/// its format's grammar, and holding no NUL a reader could cut it at.
type Conforms = fn(&[u8]) -> bool;

#[derive(Clone, Copy, Debug)]
struct Entry {
    kind: Kind,
    // A string whose raw text holds an escape, so its text must be decoded.
    escaped: bool,
    // The value's raw text, as positions in the content: a string's delimiters included.
    start: u32,
    end: u32,
    // The first entry past this one's subtree.
    after: u32,
}

impl Tree {
    /// The top value, read over `content`, the bytes the tree was built from.
    pub(crate) fn root<'a>(&'a self, content: &'a [u8]) -> Node<'a> {
        Node {
            tree: self,
            content,
            index: 0,
        }
    }
}

/// Builds a tree as a parser reads, in document order: scalars pushed, containers opened and
/// closed.
pub(crate) struct Builder<'g> {
    entries: Vec<Entry>,
    decode: Decode,
    conforms: Conforms,
    // The containers open, innermost last.
    open: Vec<usize>,
    grow: &'g mut dyn FnMut(usize) -> bool,
}

impl<'g> Builder<'g> {
    /// For content of up to 4 GiB, positions being `u32`, its strings read with `decode`, a key
    /// that doesn't decode checked with `conforms`. `grow` is asked before the tree takes more
    /// bytes, and can refuse them.
    pub(crate) fn new(
        content_bytes: usize,
        decode: Decode,
        conforms: Conforms,
        grow: &'g mut dyn FnMut(usize) -> bool,
    ) -> Result<Self, Unparsed> {
        if u32::try_from(content_bytes).is_err() {
            return Err(Unparsed::Malformed);
        }
        Ok(Self {
            entries: Vec::new(),
            decode,
            conforms,
            open: Vec::new(),
            grow,
        })
    }

    /// The innermost container open; `None` at the top.
    pub(crate) fn open_kind(&self) -> Option<Kind> {
        let &index = self.open.last()?;
        Some(self.entries[index].kind)
    }

    /// Opens a container starting at `start`; refused past 256 levels.
    pub(crate) fn open(&mut self, kind: Kind, start: usize) -> Result<(), Unparsed> {
        if self.open.len() == MAX_NESTING {
            return Err(Unparsed::Malformed);
        }
        self.open.push(self.entries.len());
        self.push(kind, false, start, start)
    }

    /// Closes the innermost container, its text ending at `end`.
    pub(crate) fn close(&mut self, end: usize) {
        let index = self.open.pop().expect("a container is open");
        let after = position(self.entries.len());
        let entry = &mut self.entries[index];
        entry.end = position(end);
        entry.after = after;
    }

    /// The tree, once one value is read whole.
    pub(crate) fn finish(self) -> Result<Tree, Unparsed> {
        if self.entries.is_empty() || !self.open.is_empty() {
            return Err(Unparsed::Malformed);
        }
        Ok(Tree {
            entries: self.entries,
            decode: self.decode,
            conforms: self.conforms,
        })
    }

    /// Pushes a scalar, its raw text from `start` to `end`.
    pub(crate) fn push(
        &mut self,
        kind: Kind,
        escaped: bool,
        start: usize,
        end: usize,
    ) -> Result<(), Unparsed> {
        if self.entries.len() == self.entries.capacity() {
            let more = self.entries.capacity().max(64);
            if !(self.grow)(more * size_of::<Entry>()) {
                return Err(Unparsed::PastBudget);
            }
            self.entries.reserve_exact(more);
        }
        let after = position(self.entries.len() + 1);
        self.entries.push(Entry {
            kind,
            escaped,
            start: position(start),
            end: position(end),
            after,
        });
        Ok(())
    }
}

// Within `u32`: the content's length is checked first, and there are fewer entries than bytes.
fn position(at: usize) -> u32 {
    u32::try_from(at).expect("the content fits u32")
}

/// What a value is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Object,
    Array,
    String,
    Number,
    Bool(bool),
    Null,
}

/// A value in a body's content.
#[derive(Clone, Copy)]
pub struct Node<'a> {
    tree: &'a Tree,
    content: &'a [u8],
    index: usize,
}

// The content can hold credentials.
impl fmt::Debug for Node<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let raw_bytes = self.raw().len();
        write!(formatter, "Node({:?}, {raw_bytes} bytes)", self.kind())
    }
}

/// One step of a path into a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step<'s> {
    Key(&'s str),
    Index(usize),
}

impl<'a> Node<'a> {
    fn entry(&self) -> Entry {
        self.tree.entries[self.index]
    }

    pub fn kind(&self) -> Kind {
        self.entry().kind
    }

    /// The value as written: a string with its delimiters and escapes, a number as written.
    pub fn raw(&self) -> &'a [u8] {
        let entry = self.entry();
        &self.content[entry.start as usize..entry.end as usize]
    }

    /// A string's text, decoded as its format writes it, borrowed where nothing needs decoding;
    /// `None` for any other value, or for a string that doesn't decode.
    pub fn text(&self) -> Option<Cow<'a, str>> {
        let Entry { kind, escaped, .. } = self.entry();
        if kind != Kind::String {
            return None;
        }
        (self.tree.decode)(self.raw(), escaped)
    }

    /// An array's items, in order; none for any other value.
    pub fn items(&self) -> impl Iterator<Item = Node<'a>> + use<'a> {
        let children = self.children(Kind::Array);
        let node = *self;
        children.map(move |index| node.at(index))
    }

    /// An object's members, keys as sent, in order, a repeated key kept; none for any other value.
    pub fn members(&self) -> impl Iterator<Item = (Node<'a>, Node<'a>)> + use<'a> {
        let mut children = self.children(Kind::Object);
        let node = *self;
        std::iter::from_fn(move || {
            let key = children.next()?;
            let value = children.next()?;
            Some((node.at(key), node.at(value)))
        })
    }

    /// The value under `key`, matched exactly; absent if only there in another letter case, or
    /// ambiguous ([`Member::Ambiguous`]).
    pub(crate) fn get(&self, key: &str) -> Member<'a> {
        let mut found = Member::Absent;
        let mut seen = false;
        for (name, value) in self.members() {
            let Some(name) = name.text() else {
                // A key outside its format's grammar may be any key: lenient readers each read it
                // their own way, too many to follow; one holding a NUL, cut there, may be a key
                // sought. Any other the grammar allows but that doesn't decode reads the same to
                // every reader, as no key sought.
                if !(self.tree.conforms)(name.raw()) {
                    return Member::Ambiguous;
                }
                continue;
            };
            // A reader cutting strings at a NUL reads `"method\u0000"` as `method`.
            if name.contains('\0') {
                return Member::Ambiguous;
            }
            if !same_but_case(&name, key) {
                continue;
            }
            if seen {
                return Member::Ambiguous;
            }
            seen = true;
            if name == key {
                found = Member::Found(value);
            }
        }
        found
    }

    /// The value at `steps` from this one: a key, ambiguous if repeated with letter case ignored,
    /// or if its object holds a key outside its format's grammar or holding a NUL; or an index
    /// into an array.
    /// Absent where a step finds nothing, or the value found is `null`.
    pub fn walk(&self, steps: &[Step<'_>]) -> Member<'a> {
        let mut member = Member::Found(*self);
        for step in steps {
            let Member::Found(node) = member else {
                return member;
            };
            member = match *step {
                Step::Key(key) => node.get(key),
                Step::Index(index) => node
                    .items()
                    .nth(index)
                    .map_or(Member::Absent, Member::Found),
            };
        }
        match member {
            Member::Found(node) if node.kind() == Kind::Null => Member::Absent,
            _ => member,
        }
    }

    fn at(&self, index: usize) -> Node<'a> {
        Node { index, ..*self }
    }

    // The indexes of a container's direct children; none unless it's of `kind`.
    fn children(&self, kind: Kind) -> Children<'a> {
        let entry = self.entry();
        Children {
            entries: &self.tree.entries,
            next: self.index + 1,
            end: if entry.kind == kind {
                entry.after as usize
            } else {
                0
            },
        }
    }
}

/// What a lookup into a body finds.
#[derive(Clone, Copy, Debug)]
pub enum Member<'a> {
    Found(Node<'a>),
    Absent,
    /// Can't be told: the key is there more than once, letter case ignored, and readers differ on
    /// which they keep; or the object holds a key outside its format's grammar, or one holding a
    /// NUL, which readers differ on.
    Ambiguous,
}

impl<'a> Member<'a> {
    /// The value, if found.
    pub fn found(self) -> Option<Node<'a>> {
        match self {
            Self::Found(node) => Some(node),
            Self::Absent | Self::Ambiguous => None,
        }
    }

    /// The value under `key` in an object found; absent or ambiguous as this is otherwise.
    pub fn get(self, key: &str) -> Member<'a> {
        match self {
            Self::Found(node) => node.get(key),
            Self::Absent | Self::Ambiguous => self,
        }
    }

    /// The text of a string found; `None` otherwise ([`Node::text`]).
    pub fn text(self) -> Option<Cow<'a, str>> {
        self.found()?.text()
    }

    pub fn is_ambiguous(self) -> bool {
        matches!(self, Self::Ambiguous)
    }
}

struct Children<'a> {
    entries: &'a [Entry],
    next: usize,
    end: usize,
}

impl Iterator for Children<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        if self.next >= self.end {
            return None;
        }
        let index = self.next;
        self.next = self.entries[index].after as usize;
        Some(index)
    }
}

fn same_but_case(left: &str, right: &str) -> bool {
    left == right || fold_case(left).eq(fold_case(right))
}

// Upper then lower case, so a letter lower-cased already but folding to another (`ſ`, `ı`) folds
// too; `İ` to `i`, as a server folding case with single-letter mappings reads it.
fn fold_case(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars()
        .map(|letter| if letter == '\u{130}' { 'i' } else { letter })
        .flat_map(char::to_uppercase)
        .flat_map(char::to_lowercase)
}
