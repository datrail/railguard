use std::borrow::Cow;

use rail_runtime::redact_credentials;

#[test]
fn userinfo_is_redacted_wherever_a_url_appears() {
    let cases = [
        (
            "GET https://u:p@host/x failed",
            "GET https://***@host/x failed",
        ),
        ("https://token@host", "https://***@host"),
        // Greedy: a password holding an `@` goes whole.
        ("http://u:p@ss@host/x", "http://***@host/x"),
        (
            "a http://u:p@h1/ b https://v:q@h2/",
            "a http://***@h1/ b https://***@h2/",
        ),
        (
            "postgres+psycopg://u:p@db:5432/x",
            "postgres+psycopg://***@db:5432/x",
        ),
        ("(http://u:p@h)", "(http://***@h)"),
        // Back to back, as the proxy's regex reads them.
        ("a://x@b://y@c", "a://***@b://***@c"),
    ];

    for (text, expected) in cases {
        assert_eq!(redact_credentials(text), expected, "{text}");
    }
}

#[test]
fn text_without_userinfo_is_left_alone() {
    for text in [
        "no url here",
        "https://host/path@x",
        "https://host /@x",
        "://u:p@host",
        "1://u:p@host",
        "user@example.com",
    ] {
        assert!(
            matches!(redact_credentials(text), Cow::Borrowed(t) if t == text),
            "{text}"
        );
    }
}

#[test]
fn the_scheme_is_bounded() {
    let long = format!("{}://u:p@host", "a".repeat(40));
    assert_eq!(
        redact_credentials(&long),
        format!("{}://***@host", "a".repeat(40))
    );

    // 31 characters of scheme with no letter among them is no scheme.
    let digits = format!("a{}://u:p@host", "1".repeat(31));
    assert_eq!(redact_credentials(&digits), digits);
}
