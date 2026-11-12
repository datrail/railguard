use rail_policy::{escape_hidden, escape_unquoted, sanitize, sanitize_quoted};

#[test]
fn test_escapes_controls_formats_and_separators() {
    let cases = [
        ("a\nb", r"a\u{a}b"),
        ("a\r\tb", r"a\u{d}\u{9}b"),
        ("\u{1b}[31m", r"\u{1b}[31m"),
        ("\u{7f}\u{85}\u{9b}", r"\u{7f}\u{85}\u{9b}"),
        ("a\u{200b}b", r"a\u{200b}b"),
        ("\u{202e}cba", r"\u{202e}cba"),
        ("\u{2066}a\u{2069}", r"\u{2066}a\u{2069}"),
        ("\u{feff}a", r"\u{feff}a"),
        ("a\u{2028}b\u{2029}c", r"a\u{2028}b\u{2029}c"),
    ];
    for (text, expected) in cases {
        assert_eq!(sanitize(text).to_string(), expected, "{text:?}");
    }
}

#[test]
fn test_keeps_what_shows() {
    for text in [
        "refund_order",
        "café",
        "e\u{301}",
        "漢字",
        "😀",
        "a\u{a0}b",
        r"a\u{a}b",
        r#"a"b\c"#,
        "",
    ] {
        assert_eq!(sanitize(text).to_string(), text, "{text:?}");
    }
}

#[test]
fn test_cuts_past_255_characters() {
    let whole = "é".repeat(255);
    assert_eq!(sanitize(&whole).to_string(), whole);

    let long = format!("{whole}é");
    assert_eq!(sanitize(&long).to_string(), format!("{whole}…"));

    let escaped = format!("{}\nrest", "a".repeat(254));
    assert_eq!(
        sanitize(&escaped).to_string(),
        format!(r"{}\u{{a}}…", "a".repeat(254))
    );
}

#[test]
fn test_quotes_escaping_quotes_backslashes_and_what_sanitize_escapes() {
    let cases = [
        ("", r#""""#),
        ("refund_order", r#""refund_order""#),
        ("café 漢字 😀", r#""café 漢字 😀""#),
        (r#"say "hi""#, r#""say \"hi\"""#),
        (r"a\b", r#""a\\b""#),
        ("ends in \\", r#""ends in \\""#),
        (r"a\u{a}b", r#""a\\u{a}b""#),
        ("a\nb", r#""a\u{a}b""#),
        ("\u{1b}[31m", r#""\u{1b}[31m""#),
        ("\u{202e}cba", r#""\u{202e}cba""#),
        ("a\u{2028}b", r#""a\u{2028}b""#),
        ("\"\\\n", r#""\"\\\u{a}""#),
    ];
    for (text, expected) in cases {
        assert_eq!(sanitize_quoted(text, 255).to_string(), expected, "{text:?}");
    }
}

#[test]
fn test_quoting_cuts_past_255_characters_inside_the_quotes() {
    let whole = "é".repeat(255);
    assert_eq!(
        sanitize_quoted(&whole, 255).to_string(),
        format!("\"{whole}\"")
    );

    let long = format!("{whole}é");
    assert_eq!(
        sanitize_quoted(&long, 255).to_string(),
        format!("\"{whole}…\"")
    );

    let escaped = format!("{}\"rest", "a".repeat(254));
    assert_eq!(
        sanitize_quoted(&escaped, 255).to_string(),
        format!(r#""{}\"…""#, "a".repeat(254))
    );
}

#[test]
fn test_escapes_unquoted_without_a_cut() {
    assert_eq!(
        escape_unquoted("say \"hi\"\n\\ \u{1b}").to_string(),
        r#"say \"hi\"\u{a}\\ \u{1b}"#
    );
    let long = "a".repeat(300);
    assert_eq!(escape_unquoted(&long).to_string(), long);
}

#[test]
fn test_escapes_only_what_hides_leaving_quotes_and_backslashes() {
    assert_eq!(
        escape_hidden("say \"hi\"\n\\ \u{1b}\u{9b}\u{202e}", None).to_string(),
        "say \"hi\"\\u{a}\\ \\u{1b}\\u{9b}\\u{202e}"
    );
    let long = "a".repeat(300);
    assert_eq!(escape_hidden(&long, None).to_string(), long);
}

#[test]
fn test_escaping_what_hides_cuts_past_max_chars() {
    assert_eq!(escape_hidden("abcd", Some(4)).to_string(), "abcd");
    assert_eq!(escape_hidden("abcde", Some(4)).to_string(), "abcd…");
    assert_eq!(escape_hidden("abc\nde", Some(4)).to_string(), "abc\\u{a}…");
}
