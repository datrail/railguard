use rail_policy::sanitize;

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
