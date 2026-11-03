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

struct Sanitized<'a>(&'a str);

impl fmt::Display for Sanitized<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut start = 0;
        for (count, (index, c)) in self.0.char_indices().enumerate() {
            if count == MAX_CHARS {
                f.write_str(&self.0[start..index])?;
                return f.write_str("…");
            }
            if is_hidden(c) {
                f.write_str(&self.0[start..index])?;
                write!(f, "\\u{{{:x}}}", u32::from(c))?;
                start = index + c.len_utf8();
            }
        }
        f.write_str(&self.0[start..])
    }
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
