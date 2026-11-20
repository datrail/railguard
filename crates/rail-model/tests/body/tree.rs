use rail_model::body::tree::{Kind, Member, Node, Step};

use super::keep;

// Strings as Rust quotes them, `?` for one that doesn't read, other values as sent.
pub(super) fn render(node: Node) -> String {
    let text = |node: Node| {
        node.text()
            .map_or("?".to_string(), |text| format!("{text:?}"))
    };
    match node.kind() {
        Kind::Object => {
            let members: Vec<String> = node
                .members()
                .map(|(key, value)| format!("{}:{}", text(key), render(value)))
                .collect();
            format!("{{{}}}", members.join(","))
        }
        Kind::Array => {
            let items: Vec<String> = node.items().map(render).collect();
            format!("[{}]", items.join(","))
        }
        Kind::String => text(node),
        _ => String::from_utf8(node.raw().to_vec()).unwrap(),
    }
}

// What a walk finds: a value rendered, or why there's none.
pub(super) fn describe(member: Member) -> String {
    match member {
        Member::Found(node) => render(node),
        Member::Absent => "absent".to_string(),
        Member::Ambiguous => "ambiguous".to_string(),
    }
}

#[test]
fn test_walks_keys_and_indexes() {
    let body = keep(
        br#"{"a": {"b": [10, {"c": "x"}, null]}, "Dup": 1, "dup": 2, "\ud800": 5, "Solo": 3,
            "n": null, "e\u0073c": 4, "\u00c9": 6, "\u00e9": 7, "foo\u017f": 8, "foos": 9,
            "\u0130d": 10, "id": 11}"#,
    );
    let root = body.json().unwrap();
    let found = |steps: &[Step]| describe(root.walk(steps));
    use Step::{Index, Key};
    // Containers' raw text too, from their opening to their closing byte.
    let list = root.walk(&[Key("a"), Key("b")]).found().unwrap();
    assert_eq!(list.raw(), br#"[10, {"c": "x"}, null]"#);
    assert_eq!(root.raw(), body.as_bytes());
    for (steps, walked, why) in [
        (&[Key("a"), Key("b"), Index(0)][..], "10", "an index"),
        (
            &[Key("a"), Key("b"), Index(1), Key("c")],
            r#""x""#,
            "into an array's object",
        ),
        (&[Key("Solo")], "3", "past a lone surrogate"),
        (&[Key("esc")], "4", "a key written with an escape"),
        (&[Key("a"), Key("b"), Index(2)], "absent", "null"),
        (&[Key("n")], "absent", "null"),
        (&[Key("a"), Key("b"), Index(3)], "absent", "past the end"),
        (&[Key("solo")], "absent", "only there in another case"),
        (&[Key("a"), Index(0)], "absent", "an index into an object"),
        (
            &[Key("a"), Key("b"), Key("0")],
            "absent",
            "a key into an array",
        ),
        (&[Key("missing")], "absent", "absent"),
        (&[Key("n"), Key("x")], "absent", "into null"),
        (&[Key("dup")], "ambiguous", "repeated but for case"),
        (&[Key("Dup")], "ambiguous", "repeated but for case"),
        (
            &[Key("é")],
            "ambiguous",
            "repeated but for case, past ASCII",
        ),
        (
            &[Key("foos")],
            "ambiguous",
            "repeated but for case, ſ folding to s",
        ),
        (
            &[Key("id")],
            "ambiguous",
            "repeated but for case, İ folding to i",
        ),
        (
            &[Key("dup"), Key("x")],
            "ambiguous",
            "a step past an ambiguous one",
        ),
    ] {
        assert_eq!(found(steps), walked, "{why}");
    }
}
