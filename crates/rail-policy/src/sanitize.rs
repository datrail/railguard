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

/// `text` escaped as [`sanitize`] escapes it, `"` and `\` by a backslash too, between double
/// quotes; cut at `max_chars` inside them, ending in `…`. Each escape reads back to one character.
pub fn sanitize_quoted(text: &str, max_chars: usize) -> impl fmt::Display + '_ {
    Quoted(text, max_chars)
}

/// `text` escaped as [`sanitize_quoted`] escapes it, with no quotes and no cut.
pub fn sanitize_unquoted(text: &str) -> impl fmt::Display + '_ {
    Unquoted(text)
}

/// `text` escaped as [`sanitize`] escapes it, `"` and `\` left as they are, for a channel that
/// encodes them itself, such as a JSON string; cut at `max_chars`, if given, ending in `…`.
pub fn sanitize_for_json(text: &str, max_chars: Option<usize>) -> impl fmt::Display + '_ {
    ForJson(text, max_chars)
}

struct Sanitized<'a>(&'a str);

impl fmt::Display for Sanitized<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_escaped(f, self.0, false, Some(MAX_CHARS))
    }
}

struct Quoted<'a>(&'a str, usize);

impl fmt::Display for Quoted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"")?;
        write_escaped(f, self.0, true, Some(self.1))?;
        f.write_str("\"")
    }
}

struct ForJson<'a>(&'a str, Option<usize>);

impl fmt::Display for ForJson<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_escaped(f, self.0, false, self.1)
    }
}

struct Unquoted<'a>(&'a str);

impl fmt::Display for Unquoted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_escaped(f, self.0, true, None)
    }
}

fn write_escaped(
    f: &mut fmt::Formatter<'_>,
    text: &str,
    quotes: bool,
    cut: Option<usize>,
) -> fmt::Result {
    let mut start = 0;
    for (count, (index, c)) in text.char_indices().enumerate() {
        if cut == Some(count) {
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
