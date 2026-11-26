use rail_model::agent::AgentId;
use rail_model::capture::{
    Combine, ContextKey, KeyFunction, Measure, MeasureFunction, OnMaxGroups, Plan, parse_plan,
};
use rail_model::condition::{BodyPath, BodyStep, Condition, Path, RequestPath};
use rail_model::schema_version::SchemaVersion;
use serde::Deserialize;
use serde_json::Value;

const VECTORS: &str = include_str!("../../../../vectors/capture/plan.json");

#[derive(Deserialize)]
struct Vectors {
    plan: Value,
}

fn vectors() -> Vectors {
    serde_json::from_str(VECTORS).unwrap()
}

fn read_example() -> Plan {
    parse_plan(&vectors().plan.to_string()).unwrap()
}

#[test]
fn test_reads_the_worked_example_as_written() {
    let plan = read_example();
    let body = |keys: &[&str]| {
        let steps = keys
            .iter()
            .map(|key| BodyStep::Key((*key).into()))
            .collect();
        Path::Request(RequestPath::Body(BodyPath(steps)))
    };

    assert_eq!(plan.schema_version, SchemaVersion { major: 1, minor: 0 });
    assert_eq!(
        plan.content_hash,
        "sha256:fb3733bbe47258024770ac2cc2521f60c5709ddc2a31b87744ed1c213229d77c"
    );
    assert_eq!(
        plan.agent,
        AgentId {
            host_id: "host-7f3c".into(),
            sandbox_name: "agent-finretail".into(),
        }
    );
    assert_eq!(plan.plan_version, 7);
    assert_eq!(plan.redaction_version, 1);
    assert_eq!(plan.window_seconds, 900);
    assert_eq!(plan.budget.max_groups_per_window, 1000);
    assert_eq!(plan.budget.max_steps_per_window, 10000);
    assert_eq!(plan.budget.on_max_groups, OnMaxGroups::CoarsenToCatchAll);
    assert_eq!(plan.budget.max_unsent_windows, 8);
    let [entry] = plan.entries.as_slice() else {
        panic!("{:?}", plan.entries);
    };
    assert_eq!(entry.id, "e1");
    assert_eq!(entry.endpoint_id, None);
    assert_eq!(
        entry.record.context_keys,
        [ContextKey::Named {
            name: "destination".into(),
            path: body(&["params", "arguments", "repo"]),
            field_type: "repository".into(),
            function: KeyFunction::None,
        }]
    );
    assert_eq!(
        entry.record.measures,
        [
            Measure::CallCount,
            Measure::Named {
                name: "request_bytes".into(),
                combine: Combine::Sum,
                function: MeasureFunction::SizeInBytes,
                paths: vec![body(&[])],
            },
            Measure::Named {
                name: "argument_bytes".into(),
                combine: Combine::Sum,
                function: MeasureFunction::SizeInBytes,
                paths: vec![body(&["params", "arguments", "content"])],
            },
        ]
    );
    assert_eq!(plan.catch_all.id, "catch_all");
    assert_eq!(
        plan.catch_all.record.context_keys,
        [
            ContextKey::Host,
            ContextKey::PathPrefix { segments: 1 },
            ContextKey::Method,
            ContextKey::Operation,
        ]
    );
    assert_eq!(
        plan.catch_all.record.measures,
        [
            Measure::CallCount,
            Measure::Named {
                name: "request_bytes".into(),
                combine: Combine::Sum,
                function: MeasureFunction::SizeInBytes,
                paths: vec![body(&[])],
            },
        ]
    );
}

// What serde_json can't hold fails the condition, as in a bundle, not the plan.
#[test]
fn test_reads_a_condition_past_what_a_double_holds_as_invalid() {
    let plan =
        vectors()
            .plan
            .to_string()
            .replacen(r#""value":"api.github.com""#, r#""value":1e400"#, 1);

    let plan = parse_plan(&plan).unwrap();

    assert!(matches!(
        plan.entries[0].conditions[0],
        Condition::Invalid(_)
    ));
}

#[test]
fn test_sanitizes_what_a_refusal_quotes() {
    let plan = vectors().plan.to_string().replacen(
        r#""name":"destination""#,
        r#""name":"a\n\u001b[31m""#,
        1,
    );

    let why = parse_plan(&plan).unwrap_err().to_string();

    assert!(!why.contains(['\n', '\u{1b}']), "{why:?}");
}

// Passed over unread, as anywhere else in the plan, so what serde_json can't hold doesn't refuse it.
#[test]
fn test_passes_over_a_key_or_measures_unknown_field_unread() {
    for (from, to) in [
        (
            r#""field_type":"repository""#,
            r#""field_type":"repository","x":1e400"#,
        ),
        (
            r#"{"name":"call_count"}"#,
            r#"{"name":"call_count","x":1e400}"#,
        ),
        (r#"{"name":"host"}"#, r#"{"name":"host","x":1e400}"#),
    ] {
        let plan = vectors().plan.to_string().replacen(from, to, 1);

        assert!(parse_plan(&plan).is_ok(), "{to}");
    }
}
