use std::collections::HashMap;

use apigee_callout_proto::apigee::{FlowVariable, MessageContext, Request, Strings, flow_variable};
use rail_apigee::{build_answer, read_path};
use rail_runtime::Answer;

#[test]
fn the_path_is_the_uri_without_its_query_percent_decoded() {
    let cases = [
        ("/mcp", "/mcp"),
        ("/mcp?session=1", "/mcp"),
        ("/a%20b/%2Fc?x=%20", "/a b//c"),
        // As Python's `unquote`: an invalid escape is kept, invalid UTF-8 replaced.
        ("/a%zz", "/a%zz"),
        ("/a%ff", "/a\u{fffd}"),
        ("", "/"),
        ("?only=query", "/"),
    ];
    for (uri, path) in cases {
        assert_eq!(read_path(uri), path, "{uri:?}");
    }
}

fn strings(values: &[&str]) -> Strings {
    Strings {
        strings: values.iter().map(|value| (*value).to_owned()).collect(),
    }
}

fn text(value: &str) -> FlowVariable {
    FlowVariable {
        value: Some(flow_variable::Value::String(value.to_owned())),
    }
}

fn received() -> MessageContext {
    MessageContext {
        request: Some(Request {
            uri: "/mcp?x=1".to_owned(),
            verb: "POST".to_owned(),
            content: "{}".to_owned(),
            headers: HashMap::from([
                ("X-Rail".to_owned(), strings(&["ticket"])),
                ("x-rail-status".to_owned(), strings(&["ok"])),
                ("x-railway".to_owned(), strings(&["kept"])),
                ("X_Rail".to_owned(), strings(&["ticket"])),
                ("accept".to_owned(), strings(&["a", "b"])),
            ]),
            form_params: HashMap::from([("f".to_owned(), strings(&["1"]))]),
            query_params: HashMap::from([("x".to_owned(), strings(&["1"]))]),
            ..Request::default()
        }),
        additional_flow_variables: HashMap::from([("other".to_owned(), text("v"))]),
        ..MessageContext::default()
    }
}

// Only what Apigee should change: the x-rail headers and `_` names removed, and our flow variables.
fn expected(flow: &[(&str, &str)]) -> MessageContext {
    let mut expected = received();
    let request = expected.request.as_mut().unwrap();
    request.headers = HashMap::from([
        ("x-rail".to_owned(), Strings::default()),
        ("X-Rail".to_owned(), Strings::default()),
        ("x-rail-status".to_owned(), Strings::default()),
        ("X_Rail".to_owned(), Strings::default()),
    ]);
    request.form_params.clear();
    request.query_params.clear();
    expected.additional_flow_variables = flow
        .iter()
        .map(|(name, value)| ((*name).to_owned(), text(value)))
        .collect();
    expected
}

#[test]
fn the_answer_removes_the_x_rail_headers_and_underscored_names_and_sets_the_decision() {
    assert_eq!(
        build_answer(received(), Answer::Forward),
        expected(&[("rail.decision", "allow")])
    );
    assert_eq!(
        build_answer(received(), Answer::Deny),
        expected(&[
            ("rail.decision", "refuse"),
            ("rail.status", "403"),
            ("rail.body", r#"{"error": "denied by policy"}"#),
        ])
    );
    assert_eq!(
        build_answer(received(), Answer::CannotApply),
        expected(&[
            ("rail.decision", "refuse"),
            ("rail.status", "503"),
            (
                "rail.body",
                r#"{"error": "policy ruleset cannot be applied"}"#
            ),
        ])
    );
}
