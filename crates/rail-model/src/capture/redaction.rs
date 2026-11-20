//! RailMon's redaction rule for a URL's path, `asp_redaction`'s `redact_path_segment`, ported.
//! Rail Center writes a plan's paths redacted by it, so a call's path is redacted the same way
//! before the two are compared. Kept in step by `vectors/capture/redaction.json`.

use std::borrow::Cow;

/// The version of the rule this port implements; a plan names the one it was written with.
pub const REDACTION_VERSION: u64 = 1;

/// What a segment that could be a key reads as.
pub const REDACTED: &str = "[redacted]";

/// A path after the rule: empty segments dropped, each other redacted, joined with a leading `/`,
/// and `/` for none.
pub fn redact_path(path: &str) -> String {
    let segments: Vec<Cow<str>> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(redact_segment)
        .collect();
    format!("/{}", segments.join("/"))
}

// The key test reads the brackets decoded, so a second pass can't push a segment past 20.
fn redact_segment(segment: &str) -> Cow<'_, str> {
    let raw = segment
        .replace("%5D", "]")
        .replace("%5d", "]")
        .replace("%5B", "[")
        .replace("%5b", "[");
    if raw == REDACTED || looks_like_secret_segment(&raw) {
        return Cow::Borrowed(REDACTED);
    }
    if raw.contains(['[', ']']) {
        return Cow::Owned(raw.replace('[', "%5B").replace(']', "%5D"));
    }
    Cow::Owned(raw)
}

// Python's `len` counts code points, as `chars` does.
fn looks_like_secret_segment(segment: &str) -> bool {
    is_secret_token(segment) || segment.chars().count() >= 20
}

/// Whether the whole value is a credential in a shape some vendor issues.
fn is_secret_token(value: &str) -> bool {
    SECRET_SHAPES.iter().any(|shape| shape.matches(value))
}

/// One of `asp_redaction`'s `SECRET_TOKEN_RES`: a regular expression, matched whole, that is a
/// sequence of literal choices and character runs.
struct Shape(&'static [Part]);

enum Part {
    OneOf(&'static [&'static str]),
    Run {
        class: fn(u8) -> bool,
        min: usize,
        max: Option<usize>,
    },
}

impl Shape {
    // Greedy runs, which match as the regular expression does: no run's class holds the
    // character after it.
    fn matches(&self, value: &str) -> bool {
        let mut rest = value.as_bytes();
        for part in self.0 {
            match part {
                Part::OneOf(choices) => {
                    let Some(choice) = choices
                        .iter()
                        .find(|choice| rest.starts_with(choice.as_bytes()))
                    else {
                        return false;
                    };
                    rest = &rest[choice.len()..];
                }
                Part::Run { class, min, max } => {
                    let run = rest
                        .iter()
                        .take(max.unwrap_or(usize::MAX))
                        .take_while(|byte| class(**byte))
                        .count();
                    if run < *min {
                        return false;
                    }
                    rest = &rest[run..];
                }
            }
        }
        rest.is_empty()
    }
}

const fn literal(text: &'static [&'static str]) -> Part {
    Part::OneOf(text)
}

const fn at_least(class: fn(u8) -> bool, min: usize) -> Part {
    Part::Run {
        class,
        min,
        max: None,
    }
}

const fn exactly(class: fn(u8) -> bool, count: usize) -> Part {
    Part::Run {
        class,
        min: count,
        max: Some(count),
    }
}

// `[A-Za-z0-9]`
fn alphanumeric(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

// `[A-Za-z0-9_-]`
fn alphanumeric_underscore_dash(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

// `[A-Za-z0-9_]`
fn alphanumeric_underscore(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

// `[A-Za-z0-9-]`
fn alphanumeric_dash(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

// `[a-f0-9]`
fn lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

// `[0-9]`
fn digit(byte: u8) -> bool {
    byte.is_ascii_digit()
}

// `[0-9A-Z]`
fn digit_or_upper(byte: u8) -> bool {
    byte.is_ascii_digit() || byte.is_ascii_uppercase()
}

// In `SECRET_TOKEN_RES`'s order, each with its expression.
const SECRET_SHAPES: [Shape; 14] = [
    // `sk-[A-Za-z0-9_-]{8,}`: OpenAI, Anthropic and lookalikes
    Shape(&[literal(&["sk-"]), at_least(alphanumeric_underscore_dash, 8)]),
    // `(?:sk|pk|rk)_(?:live|test)_[A-Za-z0-9]{20,}`: Stripe
    Shape(&[
        literal(&["sk_", "pk_", "rk_"]),
        literal(&["live_", "test_"]),
        at_least(alphanumeric, 20),
    ]),
    // `gh[pousr]_[A-Za-z0-9]{12,}`: GitHub
    Shape(&[
        literal(&["ghp_", "gho_", "ghu_", "ghs_", "ghr_"]),
        at_least(alphanumeric, 12),
    ]),
    // `github_pat_[A-Za-z0-9_]{20,}`
    Shape(&[
        literal(&["github_pat_"]),
        at_least(alphanumeric_underscore, 20),
    ]),
    // `glpat-[A-Za-z0-9_-]{6,}`: GitLab
    Shape(&[
        literal(&["glpat-"]),
        at_least(alphanumeric_underscore_dash, 6),
    ]),
    // `xox[baprs]-[A-Za-z0-9-]{10,}`: Slack
    Shape(&[
        literal(&["xoxb-", "xoxa-", "xoxp-", "xoxr-", "xoxs-"]),
        at_least(alphanumeric_dash, 10),
    ]),
    // `xapp-[0-9]-[A-Za-z0-9-]{10,}`
    Shape(&[
        literal(&["xapp-"]),
        exactly(digit, 1),
        literal(&["-"]),
        at_least(alphanumeric_dash, 10),
    ]),
    // `npm_[A-Za-z0-9]{30,}`
    Shape(&[literal(&["npm_"]), at_least(alphanumeric, 30)]),
    // `shpat_[a-f0-9]{20,}`: Shopify
    Shape(&[literal(&["shpat_"]), at_least(lower_hex, 20)]),
    // `hf_[A-Za-z0-9]{30,}`: Hugging Face
    Shape(&[literal(&["hf_"]), at_least(alphanumeric, 30)]),
    // `dop_v1_[a-f0-9]{32,}`: DigitalOcean
    Shape(&[literal(&["dop_v1_"]), at_least(lower_hex, 32)]),
    // `AIza[0-9A-Za-z_-]{30,}`: Google API key
    Shape(&[
        literal(&["AIza"]),
        at_least(alphanumeric_underscore_dash, 30),
    ]),
    // `ya29\.[A-Za-z0-9_-]{20,}`: Google OAuth
    Shape(&[
        literal(&["ya29."]),
        at_least(alphanumeric_underscore_dash, 20),
    ]),
    // `A(?:KIA|SIA)[0-9A-Z]{16}`: AWS access key id
    Shape(&[literal(&["AKIA", "ASIA"]), exactly(digit_or_upper, 16)]),
];
