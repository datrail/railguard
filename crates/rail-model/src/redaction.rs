//! RailMon's redaction rule for a URL's path, `asp_redaction`'s, ported.
//! Rail Center writes a capture plan's paths redacted by it, so a call's path is redacted the same
//! way before the two are compared. Kept in step by `vectors/redaction.json`.

/// The version of the rule this port implements; a plan names the one it was written with.
pub const REDACTION_VERSION: u64 = 1;

// What a segment that could be a key reads as, and a value that isn't kept.
pub(crate) const REDACTED: &str = "[redacted]";

// A path after the rule: empty segments dropped, each other redacted, joined with a leading `/`,
// and `/` for none.
pub(crate) fn redact_path(path: &str) -> String {
    let mut redacted = String::with_capacity(path.len() + 1);
    for segment in segments(path) {
        redacted.push('/');
        push_segment(&mut redacted, segment);
    }
    if redacted.is_empty() {
        redacted.push('/');
    }
    redacted
}

// A path's segments as the rule reads them: empty ones dropped.
pub(crate) fn segments(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|segment| !segment.is_empty())
}

// The key test reads the brackets decoded, so a second pass can't push a segment past 20.
fn push_segment(redacted: &mut String, segment: &str) {
    if !segment.contains(['%', '[', ']']) {
        let secret = looks_like_secret_segment(segment);
        redacted.push_str(if secret { REDACTED } else { segment });
        return;
    }
    let raw = segment
        .replace("%5D", "]")
        .replace("%5d", "]")
        .replace("%5B", "[")
        .replace("%5b", "[");
    if raw == REDACTED || looks_like_secret_segment(&raw) {
        redacted.push_str(REDACTED);
        return;
    }
    for character in raw.chars() {
        match character {
            '[' => redacted.push_str("%5B"),
            ']' => redacted.push_str("%5D"),
            other => redacted.push(other),
        }
    }
}

// The rule counts characters, not bytes.
fn looks_like_secret_segment(segment: &str) -> bool {
    is_secret_token(segment) || segment.chars().count() >= 20
}

// Whether the whole value is a credential in a shape some vendor issues.
pub(crate) fn is_secret_token(value: &str) -> bool {
    SECRET_SHAPES.iter().any(|shape| shape.matches(value))
}

// One of `asp_redaction`'s `SECRET_TOKEN_RES`, each a literal prefix then a run of one class:
// its alternations spelled out as prefixes, all of one length, so the first that matches is the
// only one.
struct Shape {
    prefixes: &'static [&'static str],
    class: fn(u8) -> bool,
    run: Run,
}

enum Run {
    AtLeast(usize),
    Exactly(usize),
}

impl Shape {
    fn matches(&self, value: &str) -> bool {
        let Some(rest) = self
            .prefixes
            .iter()
            .find_map(|prefix| value.strip_prefix(prefix))
        else {
            return false;
        };
        // Every class is ASCII, so a run's length in bytes is in characters.
        let in_length = match self.run {
            Run::AtLeast(min) => rest.len() >= min,
            Run::Exactly(count) => rest.len() == count,
        };
        in_length && rest.bytes().all(self.class)
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

// `[0-9A-Z]`
fn digit_or_upper(byte: u8) -> bool {
    byte.is_ascii_digit() || byte.is_ascii_uppercase()
}

// In `SECRET_TOKEN_RES`'s order, each with its expression.
const SECRET_SHAPES: [Shape; 14] = [
    // `sk-[A-Za-z0-9_-]{8,}`: OpenAI, Anthropic and lookalikes
    Shape {
        prefixes: &["sk-"],
        class: alphanumeric_underscore_dash,
        run: Run::AtLeast(8),
    },
    // `(?:sk|pk|rk)_(?:live|test)_[A-Za-z0-9]{20,}`: Stripe
    Shape {
        prefixes: &[
            "sk_live_", "sk_test_", "pk_live_", "pk_test_", "rk_live_", "rk_test_",
        ],
        class: alphanumeric,
        run: Run::AtLeast(20),
    },
    // `gh[pousr]_[A-Za-z0-9]{12,}`: GitHub
    Shape {
        prefixes: &["ghp_", "gho_", "ghu_", "ghs_", "ghr_"],
        class: alphanumeric,
        run: Run::AtLeast(12),
    },
    // `github_pat_[A-Za-z0-9_]{20,}`
    Shape {
        prefixes: &["github_pat_"],
        class: alphanumeric_underscore,
        run: Run::AtLeast(20),
    },
    // `glpat-[A-Za-z0-9_-]{6,}`: GitLab
    Shape {
        prefixes: &["glpat-"],
        class: alphanumeric_underscore_dash,
        run: Run::AtLeast(6),
    },
    // `xox[baprs]-[A-Za-z0-9-]{10,}`: Slack
    Shape {
        prefixes: &["xoxb-", "xoxa-", "xoxp-", "xoxr-", "xoxs-"],
        class: alphanumeric_dash,
        run: Run::AtLeast(10),
    },
    // `xapp-[0-9]-[A-Za-z0-9-]{10,}`
    Shape {
        prefixes: &[
            "xapp-0-", "xapp-1-", "xapp-2-", "xapp-3-", "xapp-4-", "xapp-5-", "xapp-6-", "xapp-7-",
            "xapp-8-", "xapp-9-",
        ],
        class: alphanumeric_dash,
        run: Run::AtLeast(10),
    },
    // `npm_[A-Za-z0-9]{30,}`
    Shape {
        prefixes: &["npm_"],
        class: alphanumeric,
        run: Run::AtLeast(30),
    },
    // `shpat_[a-f0-9]{20,}`: Shopify
    Shape {
        prefixes: &["shpat_"],
        class: lower_hex,
        run: Run::AtLeast(20),
    },
    // `hf_[A-Za-z0-9]{30,}`: Hugging Face
    Shape {
        prefixes: &["hf_"],
        class: alphanumeric,
        run: Run::AtLeast(30),
    },
    // `dop_v1_[a-f0-9]{32,}`: DigitalOcean
    Shape {
        prefixes: &["dop_v1_"],
        class: lower_hex,
        run: Run::AtLeast(32),
    },
    // `AIza[0-9A-Za-z_-]{30,}`: Google API key
    Shape {
        prefixes: &["AIza"],
        class: alphanumeric_underscore_dash,
        run: Run::AtLeast(30),
    },
    // `ya29\.[A-Za-z0-9_-]{20,}`: Google OAuth
    Shape {
        prefixes: &["ya29."],
        class: alphanumeric_underscore_dash,
        run: Run::AtLeast(20),
    },
    // `A(?:KIA|SIA)[0-9A-Z]{16}`: AWS access key id
    Shape {
        prefixes: &["AKIA", "ASIA"],
        class: digit_or_upper,
        run: Run::Exactly(16),
    },
];
