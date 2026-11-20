//! JSON content, read into a tree.
//!
//! Read in one pass of its own: serde_json can't tell where an escaped string or a number sits in
//! the input, so it can't build a tree of positions. It checks the structure, numbers, `true`,
//! `false` and `null`; a string's contents are checked only when it's read, serde_json decoding one
//! with an escape.

use std::borrow::Cow;

use super::Unparsed;
use super::tree::{Builder, Kind, Tree};

const BOM: &[u8] = "\u{feff}".as_bytes();

/// Reads `content`, past one UTF-8 BOM, as one JSON value. `grow` is asked before the tree takes
/// more bytes, and can refuse them.
pub(crate) fn parse(content: &[u8], grow: &mut dyn FnMut(usize) -> bool) -> Result<Tree, Unparsed> {
    let mut parser = Parser {
        content,
        at: if content.starts_with(BOM) {
            BOM.len()
        } else {
            0
        },
        builder: Builder::new(content.len(), decode_string, guess_string, grow)?,
    };
    parser.run()?;
    parser.builder.finish()
}

// A JSON string's text, `raw` with its quotes; `None` for a lone surrogate, a control character, a
// bad escape or bytes not UTF-8.
fn decode_string(raw: &[u8], escaped: bool) -> Option<Cow<'_, str>> {
    if escaped {
        return serde_json::from_slice::<String>(raw).ok().map(Cow::Owned);
    }
    let inside = &raw[1..raw.len() - 1];
    if inside.iter().any(|&byte| byte < 0x20) {
        return None;
    }
    std::str::from_utf8(inside).ok().map(Cow::Borrowed)
}

// What a forgiving reader makes of a string `decode_string` refuses, `raw` with its quotes: an
// unknown escape its letter, a lone surrogate or bytes not UTF-8 U+FFFD, a control character kept.
fn guess_string(raw: &[u8]) -> String {
    let inside = &raw[1..raw.len() - 1];
    let mut text = Vec::with_capacity(inside.len());
    let mut at = 0;
    while let Some(&byte) = inside.get(at) {
        at += 1;
        if byte != b'\\' {
            text.push(byte);
            continue;
        }
        let Some(&letter) = inside.get(at) else {
            break;
        };
        at += 1;
        let decoded = match letter {
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => match code_unit(inside, at) {
                None => 'u',
                Some(unit) => {
                    at += 4;
                    if let Some(pair) = code_pair(inside, at, unit) {
                        at += 6;
                        pair
                    } else {
                        char::from_u32(unit.into()).unwrap_or(char::REPLACEMENT_CHARACTER)
                    }
                }
            },
            _ => {
                text.push(letter);
                continue;
            }
        };
        text.extend_from_slice(decoded.encode_utf8(&mut [0; 4]).as_bytes());
    }
    String::from_utf8_lossy(&text).into_owned()
}

// The four hex digits at `at`, if they're there.
fn code_unit(inside: &[u8], at: usize) -> Option<u16> {
    let digits = inside.get(at..at + 4)?;
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u16::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
}

// A high surrogate `high` and the `\u` low surrogate at `at`, as one character.
fn code_pair(inside: &[u8], at: usize, high: u16) -> Option<char> {
    if !(0xd800..0xdc00).contains(&high) || inside.get(at..at + 2)? != b"\\u" {
        return None;
    }
    let low = code_unit(inside, at + 2)?;
    char::decode_utf16([high, low]).next()?.ok()
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

struct Parser<'c, 'g> {
    content: &'c [u8],
    at: usize,
    builder: Builder<'g>,
}

impl Parser<'_, '_> {
    fn run(&mut self) -> Result<(), Unparsed> {
        let mut want = Want::Value;
        loop {
            self.skip_whitespace();
            let Some(&byte) = self.content.get(self.at) else {
                return match want {
                    Want::Next if self.builder.open_kind().is_none() => Ok(()),
                    _ => Err(Unparsed::Malformed),
                };
            };
            want = match (want, byte) {
                (Want::Next, _) if self.builder.open_kind().is_none() => {
                    return Err(Unparsed::Malformed);
                }
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
                _ => return Err(Unparsed::Malformed),
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
            _ => return Err(Unparsed::Malformed),
        }
        Ok(Want::Next)
    }

    fn in_object(&self) -> bool {
        self.builder.open_kind() == Some(Kind::Object)
    }

    fn open_container(&mut self, kind: Kind) -> Result<(), Unparsed> {
        self.builder.open(kind, self.at)?;
        self.at += 1;
        Ok(())
    }

    fn close(&mut self) -> Want {
        self.at += 1;
        self.builder.close(self.at);
        Want::Next
    }

    // Finds the closing quote: a backslash skips the byte after it, whatever it is.
    fn string(&mut self) -> Result<(), Unparsed> {
        let start = self.at;
        let mut escaped = false;
        self.at += 1;
        loop {
            let rest = self.content.get(self.at..).unwrap_or_default();
            let Some(offset) = rest.iter().position(|&byte| byte == b'"' || byte == b'\\') else {
                return Err(Unparsed::Malformed);
            };
            self.at += offset;
            if self.content[self.at] == b'"' {
                self.at += 1;
                return self.builder.push(Kind::String, escaped, start, self.at);
            }
            escaped = true;
            self.at += 2;
        }
    }

    fn literal(&mut self, word: &[u8], kind: Kind) -> Result<(), Unparsed> {
        if !self.content[self.at..].starts_with(word) {
            return Err(Unparsed::Malformed);
        }
        let start = self.at;
        self.at += word.len();
        self.builder.push(kind, false, start, self.at)
    }

    // `-? (0 | [1-9][0-9]*) (. [0-9]+)? ([eE] [+-]? [0-9]+)?`
    fn number(&mut self) -> Result<(), Unparsed> {
        let start = self.at;
        self.skip_if(|byte| byte == b'-');
        match self.content.get(self.at) {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.skip_digits();
            }
            _ => return Err(Unparsed::Malformed),
        }
        if self.skip_if(|byte| byte == b'.') && self.skip_digits() == 0 {
            return Err(Unparsed::Malformed);
        }
        if self.skip_if(|byte| byte == b'e' || byte == b'E') {
            self.skip_if(|byte| byte == b'+' || byte == b'-');
            if self.skip_digits() == 0 {
                return Err(Unparsed::Malformed);
            }
        }
        self.builder.push(Kind::Number, false, start, self.at)
    }

    fn skip_if(&mut self, accept: impl Fn(u8) -> bool) -> bool {
        let skipped = self.content.get(self.at).is_some_and(|&byte| accept(byte));
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
}
