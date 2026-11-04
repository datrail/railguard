use std::borrow::Cow;

// The proxy's `[a-zA-Z][\w+.-]{0,30}://`: bounded, so a long run that isn't a scheme costs little.
const MAX_SCHEME_CHARS: usize = 31;

/// `text` with the userinfo of every URL in it written as `***`: from `scheme://` to the last `@`
/// before a `/` or whitespace, so a password holding its own `@` goes whole.
pub fn redact_credentials(text: &str) -> Cow<'_, str> {
    let mut redacted = String::new();
    // Where the text not yet copied starts; no scheme reaches back before it.
    let mut copied = 0;
    let mut search = 0;
    while let Some(found) = text[search..].find("://") {
        let separator = search + found;
        let authority = separator + "://".len();
        search = authority;
        if !has_scheme(&text[copied..separator]) {
            continue;
        }
        let run = text[authority..]
            .find(|c: char| c == '/' || c.is_whitespace())
            .map_or(text.len(), |end| authority + end);
        if let Some(at) = text[authority..run].rfind('@') {
            redacted.push_str(&text[copied..authority]);
            redacted.push_str("***@");
            copied = authority + at + 1;
            search = copied;
        }
    }
    if copied == 0 {
        return Cow::Borrowed(text);
    }
    redacted.push_str(&text[copied..]);
    Cow::Owned(redacted)
}

// Python's `\w` and `\s` differ from these at the edges, always toward redacting more here.
// Whether `before` ends in a scheme: a letter, then letters, digits or `_+.-`, 31 at most.
fn has_scheme(before: &str) -> bool {
    before
        .chars()
        .rev()
        .take(MAX_SCHEME_CHARS)
        .take_while(|&c| c.is_alphanumeric() || matches!(c, '_' | '+' | '.' | '-'))
        .any(|c| c.is_ascii_alphabetic())
}
