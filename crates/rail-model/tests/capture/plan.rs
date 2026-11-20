use rail_model::agent::AgentId;
use rail_model::capture::{
    CatchAllKey, Combine, ContextKey, Function, Measure, OnMaxGroups, Plan, parse_plan,
};
use rail_model::condition::{BodyPath, BodyStep, Condition, Path, RequestPath};
use rail_model::schema_version::SchemaVersion;
use serde::Deserialize;
use serde_json::Value;

const VECTORS: &str = include_str!("../../../../vectors/parse_plan_conformance_cases.json");

#[derive(Deserialize)]
struct Vectors {
    plan: Value,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    #[serde(default)]
    patch: Vec<Value>,
    expect: Expect,
}

#[derive(Deserialize)]
struct Expect {
    valid: bool,
    /// The first entry's, as read.
    conditions: Option<Vec<String>>,
}

fn vectors() -> Vectors {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());
    vectors
}

// RFC 6902's add, remove and replace, as the vectors use them.
fn patch(document: &mut Value, operation: &Value) {
    let pointer = operation["path"].as_str().unwrap();
    let (parent, last) = pointer.rsplit_once('/').unwrap();
    let parent = document.pointer_mut(parent).unwrap();
    let value = operation.get("value").cloned();
    match (operation["op"].as_str().unwrap(), parent) {
        ("remove", Value::Object(fields)) => drop(fields.remove(last).unwrap()),
        ("remove", Value::Array(items)) => drop(items.remove(last.parse().unwrap())),
        ("add" | "replace", Value::Object(fields)) => {
            drop(fields.insert(last.into(), value.unwrap()))
        }
        ("add", Value::Array(items)) if last == "-" => items.push(value.unwrap()),
        ("replace", Value::Array(items)) => items[last.parse::<usize>().unwrap()] = value.unwrap(),
        (op, _) => panic!("{op} at {pointer}"),
    }
}

fn condition_kind(condition: &Condition) -> &'static str {
    match condition {
        Condition::Valid(_) => "valid",
        Condition::Unknown(_) => "unknown",
        Condition::Invalid(_) => "invalid",
    }
}

fn read_example() -> Plan {
    parse_plan(&vectors().plan.to_string()).unwrap()
}

#[test]
fn test_vectors() {
    let vectors = vectors();
    for case in &vectors.cases {
        let mut plan = vectors.plan.clone();
        case.patch
            .iter()
            .for_each(|operation| patch(&mut plan, operation));

        let read = parse_plan(&plan.to_string());

        let name = &case.name;
        match (read, case.expect.valid) {
            (Ok(plan), true) => {
                if let Some(conditions) = &case.expect.conditions {
                    let read: Vec<_> = plan.entries[0]
                        .conditions
                        .iter()
                        .map(condition_kind)
                        .collect();
                    assert_eq!(&read, conditions, "{name}");
                }
            }
            (Err(_), false) => {}
            (read, _) => panic!("{name}: {read:?}"),
        }
    }
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
    assert_eq!(plan.budget.max_steps_per_window, Some(10000));
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
        }]
    );
    assert_eq!(
        entry.record.measures,
        [
            Measure::CallCount,
            Measure::ErrorCount,
            Measure::RequestBytes,
            Measure::Named {
                name: "argument_bytes".into(),
                combine: Combine::Sum,
                function: Function::Size,
                paths: vec![body(&["params", "arguments", "content"])],
            },
        ]
    );
    assert_eq!(plan.catch_all.id, "catch_all");
    assert_eq!(
        plan.catch_all.record.context_keys,
        [
            CatchAllKey::Host,
            CatchAllKey::PathPrefix { segments: 1 },
            CatchAllKey::Method,
            CatchAllKey::Operation,
        ]
    );
    assert_eq!(
        plan.catch_all.record.measures,
        [
            Measure::CallCount,
            Measure::ErrorCount,
            Measure::RequestBytes
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
fn test_says_what_it_refuses() {
    let why = parse_plan("{}").unwrap_err().to_string();

    assert!(why.starts_with("invalid capture plan: "), "{why}");
}
