use rail_model::body::tree::{Kind, Node, Step};

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

#[test]
fn test_walks_keys_and_indexes() {
    let body = keep(
        br#"{"a": {"b": [10, {"c": "x"}, null]}, "Dup": 1, "dup": 2, "\ud800": 5, "Solo": 3,
            "n": null, "e\u0073c": 4, "\u00c9": 6, "\u00e9": 7, "foo\u017f": 8, "foos": 9,
            "\u0130d": 10, "id": 11}"#,
    );
    let root = body.json().unwrap();
    let found = |steps: &[Step]| root.walk(steps).map(render);
    use Step::{Index, Key};
    assert_eq!(
        found(&[Key("a"), Key("b"), Index(0)]).as_deref(),
        Some("10")
    );
    assert_eq!(
        found(&[Key("a"), Key("b"), Index(1), Key("c")]).as_deref(),
        Some(r#""x""#)
    );
    assert_eq!(found(&[Key("Solo")]).as_deref(), Some("3"));
    assert_eq!(found(&[Key("esc")]).as_deref(), Some("4"));
    // Containers' raw text too, from their opening to their closing byte.
    assert_eq!(
        root.walk(&[Key("a"), Key("b")]).unwrap().raw(),
        br#"[10, {"c": "x"}, null]"#
    );
    assert_eq!(root.raw(), body.as_bytes());
    for (steps, why) in [
        (&[Key("a"), Key("b"), Index(2)][..], "null"),
        (&[Key("n")], "null"),
        (&[Key("a"), Key("b"), Index(3)], "past the end"),
        (&[Key("dup")], "repeated but for case"),
        (&[Key("Dup")], "repeated but for case"),
        (&[Key("é")], "repeated but for case, past ASCII"),
        (&[Key("foos")], "repeated but for case, ſ folding to s"),
        (&[Key("id")], "repeated but for case, İ folding to i"),
        (&[Key("solo")], "only there in another case"),
        (&[Key("a"), Index(0)], "an index into an object"),
        (&[Key("a"), Key("b"), Key("0")], "a key into an array"),
        (&[Key("missing")], "absent"),
    ] {
        assert_eq!(found(steps), None, "{why}");
    }
}
