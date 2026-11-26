use std::borrow::Cow;

use rail_model::body::Unparsed;
use serde::de::IgnoredAny;

use super::keep;
use super::tree::render;

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
fn test_refuses_a_body_that_isnt_one_json_value() {
    assert_eq!(parse(b""), Err(Unparsed::Malformed));
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
