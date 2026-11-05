use std::fmt;

use unicode_general_category::{GeneralCategory, get_general_category};

// Rail Center's cap on an endpoint key: past it, a value is a payload, not a name.
const MAX_CHARS: usize = 255;

/// Text from the wire, made safe to write where a person reads it: each control, format or
/// separator character is written as `\u{…}`, everything else as it is, and the text is cut at
/// 255 characters, ending in `…`.
pub fn sanitize(text: &str) -> impl fmt::Display + '_ {
    Sanitized(text)
}

/// `text` as [`sanitize`] writes it, with `"` and `\` escaped by a backslash too, between double
/// quotes: the cut falls inside them, and each escape reads back to one character.
pub fn sanitize_quoted(text: &str) -> impl fmt::Display + '_ {
    Quoted(text)
}

/// `text` escaped as [`sanitize_quoted`] escapes it, with no quotes and no cut.
pub fn escape_unquoted(text: &str) -> impl fmt::Display + '_ {
    Unquoted(text)
}

struct Sanitized<'a>(&'a str);

impl fmt::Display for Sanitized<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_escaped(f, self.0, false, true)
    }
}

struct Quoted<'a>(&'a str);

impl fmt::Display for Quoted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"")?;
        write_escaped(f, self.0, true, true)?;
        f.write_str("\"")
    }
}

struct Unquoted<'a>(&'a str);

impl fmt::Display for Unquoted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_escaped(f, self.0, true, false)
    }
}

fn write_escaped(f: &mut fmt::Formatter<'_>, text: &str, quotes: bool, cut: bool) -> fmt::Result {
    let mut start = 0;
    for (count, (index, c)) in text.char_indices().enumerate() {
        if cut && count == MAX_CHARS {
            f.write_str(&text[start..index])?;
            return f.write_str("…");
        }
        let hidden = is_hidden(c);
        if hidden || (quotes && matches!(c, '"' | '\\')) {
            f.write_str(&text[start..index])?;
            if hidden {
                write!(f, "\\u{{{:x}}}", u32::from(c))?;
            } else {
                write!(f, "\\{c}")?;
            }
            start = index + c.len_utf8();
        }
    }
    f.write_str(&text[start..])
}

// These can end a line, or change how the rest of it reads, without showing themselves.
fn is_hidden(c: char) -> bool {
    matches!(
        get_general_category(c),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
    )
}
