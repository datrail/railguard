//! A JSON body as a tree of positions into its bytes, each string decoded only when read.
//!
//! Built in one pass of its own: serde_json can't tell where an escaped string or a number sits in
//! the input, so it can't build a tree of positions. It checks the structure, numbers, `true`,
//! `false`, `null` and the nesting; a string's contents are left to serde_json when it's read.

use std::borrow::Cow;

use super::Unparsed;

// Past this, a body isn't read.
const MAX_NESTING: usize = 256;

const BOM: &[u8] = "\u{feff}".as_bytes();

/// Every key and value in a body, in document order, each container followed by its subtree.
#[derive(Debug)]
pub(crate) struct Tree {
    entries: Vec<Entry>,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    kind: Kind,
    // A string holding a backslash, which must be decoded to read.
    escaped: bool,
    // The value's text, as positions in the body: a string with its quotes.
    start: u32,
    end: u32,
    // The first entry past this one's subtree.
    after: u32,
}

impl Tree {
    /// Reads `body`, past one UTF-8 BOM, as one JSON value. `grow` is asked before the tree takes
    /// more bytes, and can refuse them.
    pub(crate) fn parse(
        body: &[u8],
        grow: &mut dyn FnMut(usize) -> bool,
    ) -> Result<Self, Unparsed> {
        // Positions are `u32`.
        if u32::try_from(body.len()).is_err() {
            return Err(Unparsed::NotJson);
        }
        let mut parser = Parser {
            body,
            at: if body.starts_with(BOM) { BOM.len() } else { 0 },
            entries: Vec::new(),
            open: Vec::new(),
            grow,
        };
        parser.run()?;
        Ok(Self {
            entries: parser.entries,
        })
    }

    /// The top value, read over `body`, the bytes the tree was parsed from.
    pub(crate) fn root<'a>(&'a self, body: &'a [u8]) -> Node<'a> {
        Node {
            tree: self,
            body,
            index: 0,
        }
    }
}

// What may come next.
#[derive(Clone, Copy)]
enum Want {
    Value,
    ValueOrClose,
    Key,
    KeyOrClose,
    Colon,
    // A comma or a close in a container; the end past the top value.
    Next,
}

struct Parser<'b, 'g> {
    body: &'b [u8],
    at: usize,
    entries: Vec<Entry>,
    // The containers open, innermost last.
    open: Vec<usize>,
    grow: &'g mut dyn FnMut(usize) -> bool,
}

impl Parser<'_, '_> {
    fn run(&mut self) -> Result<(), Unparsed> {
        let mut want = Want::Value;
        loop {
            self.skip_whitespace();
            let Some(&byte) = self.body.get(self.at) else {
                return match want {
                    Want::Next if self.open.is_empty() => Ok(()),
                    _ => Err(Unparsed::NotJson),
                };
            };
            want = match (want, byte) {
                (Want::Next, _) if self.open.is_empty() => return Err(Unparsed::NotJson),
                (Want::Next, b',') => {
                    self.at += 1;
                    if self.in_object() {
                        Want::Key
                    } else {
                        Want::Value
                    }
                }
                (Want::Next, b']') if !self.in_object() => self.close(),
                (Want::Next, b'}') if self.in_object() => self.close(),
                (Want::ValueOrClose, b']') | (Want::KeyOrClose, b'}') => self.close(),
                (Want::Key | Want::KeyOrClose, b'"') => {
                    self.string()?;
                    Want::Colon
                }
                (Want::Colon, b':') => {
                    self.at += 1;
                    Want::Value
                }
                (Want::Value | Want::ValueOrClose, _) => self.value(byte)?,
                _ => return Err(Unparsed::NotJson),
            };
        }
    }

    fn value(&mut self, byte: u8) -> Result<Want, Unparsed> {
        match byte {
            b'{' => {
                self.open_container(Kind::Object)?;
                return Ok(Want::KeyOrClose);
            }
            b'[' => {
                self.open_container(Kind::Array)?;
                return Ok(Want::ValueOrClose);
            }
            b'"' => self.string()?,
            b't' => self.literal(b"true", Kind::Bool(true))?,
            b'f' => self.literal(b"false", Kind::Bool(false))?,
            b'n' => self.literal(b"null", Kind::Null)?,
            b'-' | b'0'..=b'9' => self.number()?,
            _ => return Err(Unparsed::NotJson),
        }
        Ok(Want::Next)
    }

    fn in_object(&self) -> bool {
        self.open
            .last()
            .is_some_and(|&index| self.entries[index].kind == Kind::Object)
    }

    fn open_container(&mut self, kind: Kind) -> Result<(), Unparsed> {
        if self.open.len() == MAX_NESTING {
            return Err(Unparsed::NotJson);
        }
        self.open.push(self.entries.len());
        self.push(kind, false, self.at, 0)?;
        self.at += 1;
        Ok(())
    }

    fn close(&mut self) -> Want {
        let index = self.open.pop().expect("a container is open");
        self.at += 1;
        let after = position(self.entries.len());
        let entry = &mut self.entries[index];
        entry.end = position(self.at);
        entry.after = after;
        Want::Next
    }

    // Finds the closing quote: a backslash skips the byte after it, whatever it is.
    fn string(&mut self) -> Result<(), Unparsed> {
        let start = self.at;
        let mut escaped = false;
        self.at += 1;
        loop {
            let rest = self.body.get(self.at..).unwrap_or_default();
            let Some(offset) = rest.iter().position(|&byte| byte == b'"' || byte == b'\\') else {
                return Err(Unparsed::NotJson);
            };
            self.at += offset;
            if self.body[self.at] == b'"' {
                self.at += 1;
                return self.push(Kind::String, escaped, start, self.at);
            }
            escaped = true;
            self.at += 2;
        }
    }

    fn literal(&mut self, word: &[u8], kind: Kind) -> Result<(), Unparsed> {
        if !self.body[self.at..].starts_with(word) {
            return Err(Unparsed::NotJson);
        }
        let start = self.at;
        self.at += word.len();
        self.push(kind, false, start, self.at)
    }

    // `-? (0 | [1-9][0-9]*) (. [0-9]+)? ([eE] [+-]? [0-9]+)?`
    fn number(&mut self) -> Result<(), Unparsed> {
        let start = self.at;
        self.skip_if(|byte| byte == b'-');
        match self.body.get(self.at) {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.skip_digits();
            }
            _ => return Err(Unparsed::NotJson),
        }
        if self.skip_if(|byte| byte == b'.') && self.skip_digits() == 0 {
            return Err(Unparsed::NotJson);
        }
        if self.skip_if(|byte| byte == b'e' || byte == b'E') {
            self.skip_if(|byte| byte == b'+' || byte == b'-');
            if self.skip_digits() == 0 {
                return Err(Unparsed::NotJson);
            }
        }
        self.push(Kind::Number, false, start, self.at)
    }

    fn skip_if(&mut self, accept: impl Fn(u8) -> bool) -> bool {
        let skipped = self.body.get(self.at).is_some_and(|&byte| accept(byte));
        if skipped {
            self.at += 1;
        }
        skipped
    }

    fn skip_digits(&mut self) -> usize {
        let start = self.at;
        while self.skip_if(|byte| byte.is_ascii_digit()) {}
        self.at - start
    }

    fn skip_whitespace(&mut self) {
        while self.skip_if(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r')) {}
    }

    fn push(
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

// Within `u32`: the body's length is checked before parsing, and there are fewer entries than bytes.
fn position(at: usize) -> u32 {
    u32::try_from(at).expect("the body fits u32")
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

/// A value in a parsed body.
#[derive(Clone, Copy)]
pub struct Node<'a> {
    tree: &'a Tree,
    body: &'a [u8],
    index: usize,
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

    /// The value as sent: a string with its quotes and escapes, a number as written.
    pub fn raw(&self) -> &'a [u8] {
        let entry = self.entry();
        &self.body[entry.start as usize..entry.end as usize]
    }

    /// A string's text; `None` for any other value, or for a string JSON can't hold (a lone
    /// surrogate, a control character, a bad escape, bytes not UTF-8).
    pub fn text(&self) -> Option<Cow<'a, str>> {
        let Entry { kind, escaped, .. } = self.entry();
        if kind != Kind::String {
            return None;
        }
        let raw = self.raw();
        if escaped {
            return serde_json::from_slice::<String>(raw).ok().map(Cow::Owned);
        }
        let inside = &raw[1..raw.len() - 1];
        if inside.iter().any(|&byte| byte < 0x20) {
            return None;
        }
        std::str::from_utf8(inside).ok().map(Cow::Borrowed)
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

    // The value under `key`: `None` if absent, or repeated, letter case ignored. A key is compared
    // as it reads; one that doesn't read matches nothing.
    pub(crate) fn get(&self, key: &str) -> Option<Node<'a>> {
        match self.look_up(key) {
            Member::Found(value) => Some(value),
            _ => None,
        }
    }

    // Whether `key` is there more than once, letter case ignored: what it holds can't be told,
    // since readers differ on which they keep.
    pub(crate) fn repeats(&self, key: &str) -> bool {
        matches!(self.look_up(key), Member::Repeated)
    }

    fn look_up(&self, key: &str) -> Member<'a> {
        let mut found = Member::Absent;
        let mut seen = false;
        for (name, value) in self.members() {
            let Some(name) = name.text() else {
                continue;
            };
            if !same_but_case(&name, key) {
                continue;
            }
            if seen {
                return Member::Repeated;
            }
            seen = true;
            if name == key {
                found = Member::Found(value);
            }
        }
        found
    }

    /// The value at `steps` from this one: a key, absent if repeated with letter case ignored, or an
    /// index into an array; `None` where a step finds nothing, or the value found is `null`. A key
    /// is compared as it reads; one that doesn't read matches nothing.
    pub fn walk(&self, steps: &[Step<'_>]) -> Option<Node<'a>> {
        let mut node = *self;
        for step in steps {
            node = match *step {
                Step::Key(key) => node.get(key)?,
                Step::Index(index) => node.items().nth(index)?,
            };
        }
        Some(node).filter(|node| node.kind() != Kind::Null)
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

enum Member<'a> {
    Absent,
    Found(Node<'a>),
    Repeated,
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

// Upper then lower case, so a letter lower-cased already but folding to another (`ſ`, `ı`) folds too.
fn same_but_case(left: &str, right: &str) -> bool {
    left == right || fold_case(left).eq(fold_case(right))
}

fn fold_case(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars()
        .flat_map(char::to_uppercase)
        .flat_map(char::to_lowercase)
}
