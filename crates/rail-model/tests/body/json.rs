use std::borrow::Cow;

use rail_model::body::Unparsed;
use rail_model::body::tree::Step::{self, Key};
use serde::de::IgnoredAny;

use super::keep;
use super::tree::{describe, render};

fn parse(body: &[u8]) -> Result<String, Unparsed> {
    keep(body).json().map(render)
}

#[test]
fn test_keeps_every_value_in_order_as_sent() {
    for (body, tree) in [
        (
            &br#"{"a": [1, -0.5E+3, 1e400, true, false, null, "x"], "b": {}}"#[..],
            r#"{"a":[1,-0.5E+3,1e400,true,false,null,"x"],"b":{}}"#,
        ),
        (b" \t\n\r[ ] ", "[]"),
        (br#"{"a": 1, "a": 2, "A": 3}"#, r#"{"a":1,"a":2,"A":3}"#),
        (br#""a b""#, r#""a b""#),
        (b"-0", "-0"),
        ("\u{feff}{}".as_bytes(), "{}"),
        (br#"{"q": "\"[[{"}"#, r#"{"q":"\"[[{"}"#),
        (br#"{"a\u0062": "\u00e9\n"}"#, r#"{"ab":"é\n"}"#),
    ] {
        assert_eq!(
            parse(body),
            Ok(tree.to_string()),
            "{}",
            String::from_utf8_lossy(body)
        );
    }
}

// A string's contents are checked only when it's read.
// Copied only where an escape must be decoded.
#[test]
fn test_borrows_a_strings_text_with_no_escape() {
    let plain = keep(br#""a b""#);
    assert!(matches!(
        plain.json().unwrap().text(),
        Some(Cow::Borrowed("a b"))
    ));
    let escaped = keep(br#""a\nb""#);
    assert!(matches!(escaped.json().unwrap().text(), Some(Cow::Owned(text)) if text == "a\nb"));
}

#[test]
fn test_reads_a_string_json_cant_hold_as_no_text() {
    for body in [
        &br#"{"a": "\ud800", "b": 1}"#[..],
        b"{\"a\": \"\x01\", \"b\": 1}",
        b"{\"a\": \"\xff\", \"b\": 1}",
    ] {
        assert_eq!(
            parse(body),
            Ok(r#"{"a":?,"b":1}"#.to_string()),
            "{}",
            String::from_utf8_lossy(body)
        );
    }
}

// Lenient readers each read a key outside the grammar their own way, so it may be any key.
#[test]
fn test_reads_no_key_in_an_object_holding_a_key_not_json() {
    for body in [
        &br#"{"o": {"a": 1, "\q": 2}, "b": 3}"#[..],
        br#"{"o": {"a": 1, "\u00ez": 2}, "b": 3}"#,
        b"{\"o\": {\"a\": 1, \"\t\": 2}, \"b\": 3}",
        b"{\"o\": {\"a\": 1, \"\xc1\xad\": 2}, \"b\": 3}",
    ] {
        let kept = keep(body);
        let root = kept.json().unwrap();
        let found = |steps: &[Step]| describe(root.walk(steps));
        let shown = String::from_utf8_lossy(body);
        assert_eq!(found(&[Key("o"), Key("a")]), "ambiguous", "{shown}");
        assert_eq!(found(&[Key("b")]), "3", "{shown}");
    }
    // A lone surrogate is JSON, a surrogate or U+FFFD to every reader, whatever escapes are beside it.
    let kept = keep(br#"{"\ud800\"\\\/\b\f\n\r\t\u00e9": 1, "a": 2}"#);
    assert_eq!(describe(kept.json().unwrap().walk(&[Key("a")])), "2");
}

#[test]
fn test_refuses_a_body_that_isnt_one_json_value() {
    let bom = "\u{feff}";
    for body in [
        "",
        r#"{"a":}"#,
        r#"{"a":1,}"#,
        "[1,]",
        "[1 2]",
        "{1:2}",
        r#"{"a":1]"#,
        "truex",
        "nul",
        "{} {}",
        r#""abc"#,
        r#""abc\""#,
        "{'a':1}",
        &format!("{bom}{bom}{{}}"),
    ] {
        assert_eq!(parse(body.as_bytes()), Err(Unparsed::Malformed), "{body:?}");
    }
}

#[test]
fn test_refuses_a_body_nested_past_256_levels() {
    let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    assert!(parse(nested(256).as_bytes()).is_ok());
    assert_eq!(parse(nested(257).as_bytes()), Err(Unparsed::Malformed));
}

// Wherever every string is one JSON can hold, a body is JSON exactly when serde_json reads it.
#[test]
fn test_reads_the_grammar_as_serde_json_does() {
    let tokens = [
        "{", "}", "[", "]", ",", ":", "\"a\"", "\"", "0", "1", "-", ".", "e", "E", "+", "true",
        "false", "null", "tru", " ",
    ];
    // xorshift64: the same bodies every run.
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = |below: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % below as u64) as usize
    };
    let mut read = 0;
    for _ in 0..200_000 {
        let count = 1 + next(8);
        let body: String = (0..count).map(|_| tokens[next(tokens.len())]).collect();
        let ours = keep(body.as_bytes()).json().is_ok();
        let theirs = serde_json::from_slice::<IgnoredAny>(body.as_bytes()).is_ok();
        assert_eq!(ours, theirs, "{body:?}");
        read += usize::from(ours);
    }
    // Enough of them JSON for the comparison to mean something.
    assert!(read > 1_000, "{read}");
}
