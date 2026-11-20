use rail_model::capture::{
    Agent, CatchAllKey, Combine, ContextKey, Function, Measure, OnMaxGroups, PlanReading,
    PlanState, Refused, parse_plan,
};
use rail_model::condition::{BodyPath, BodyStep, Condition, Path, RequestPath};
use serde::Deserialize;
use serde_json::Value;

const VECTORS: &str = include_str!("../../../../vectors/parse_plan_conformance_cases.json");

#[derive(Deserialize)]
struct Vectors {
    agent: Value,
    plan: Value,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    bytes: Option<String>,
    #[serde(default)]
    patch: Vec<Value>,
    expect: Value,
}

fn vectors() -> Vectors {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());
    vectors
}

fn agent(vectors: &Vectors) -> Agent {
    Agent {
        host_id: vectors.agent["host_id"].as_str().unwrap().to_owned(),
        sandbox_name: vectors.agent["sandbox_name"].as_str().unwrap().to_owned(),
    }
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

fn read(vectors: &Vectors, case: &Case) -> Result<PlanReading, Refused> {
    let bytes = case.bytes.clone().unwrap_or_else(|| {
        let mut plan = vectors.plan.clone();
        case.patch
            .iter()
            .for_each(|operation| patch(&mut plan, operation));
        plan.to_string()
    });
    parse_plan(bytes.as_bytes(), &agent(vectors))
}

fn condition_kind(condition: &Condition) -> &'static str {
    match condition {
        Condition::Valid(_) => "valid",
        Condition::Unknown(_) => "unknown",
        Condition::Invalid(_) => "invalid",
    }
}

#[test]
fn test_vectors() {
    let vectors = vectors();
    for case in &vectors.cases {
        let name = &case.name;
        let expect = &case.expect;
        let reading = match read(&vectors, case) {
            Err(refused) => {
                let expected = match expect["refused"].as_str() {
                    Some("not_json") => Refused::NotJson,
                    Some("no_content_hash") => Refused::NoContentHash,
                    Some("other_agent") => Refused::OtherAgent,
                    _ => panic!("{name}: refused as {refused:?}"),
                };
                assert_eq!(refused, expected, "{name}");
                continue;
            }
            Ok(reading) => reading,
        };
        assert_eq!(
            reading.content_hash.as_str(),
            vectors.plan["content_hash"],
            "{name}"
        );
        if let Some(version) = expect.get("plan_version") {
            assert_eq!(reading.plan_version, version.as_u64(), "{name}");
        }
        match (expect["reading"].as_str(), &reading.state) {
            (Some("valid"), PlanState::Valid(plan)) => {
                if let Some(conditions) = expect.get("conditions") {
                    let read: Vec<_> = plan.entries[0]
                        .conditions
                        .iter()
                        .map(condition_kind)
                        .collect();
                    assert_eq!(Value::from(read), *conditions, "{name}");
                }
            }
            (Some("unknown"), PlanState::Unknown) => {}
            (Some("redaction_mismatch"), PlanState::RedactionMismatch) => {}
            (Some("invalid"), PlanState::Invalid(why)) => {
                let at = expect["at"].as_str().unwrap();
                assert!(why.starts_with(&format!("{at}:")), "{name}: {why}");
            }
            (_, state) => panic!("{name}: read as {state:?}"),
        }
    }
}

#[test]
fn test_reads_the_worked_example_as_written() {
    let vectors = vectors();
    let reading = parse_plan(vectors.plan.to_string().as_bytes(), &agent(&vectors)).unwrap();
    let PlanState::Valid(plan) = reading.state else {
        panic!("{:?}", reading.state);
    };
    let body = |keys: &[&str]| {
        let steps = keys
            .iter()
            .map(|key| BodyStep::Key((*key).into()))
            .collect();
        Path::Request(RequestPath::Body(BodyPath(steps)))
    };

    assert_eq!(plan.window_seconds, 900);
    assert_eq!(plan.budget.max_groups_per_window, 1000);
    assert_eq!(plan.budget.max_steps_per_window, Some(10000));
    assert_eq!(plan.budget.on_max_groups, OnMaxGroups::CoarsenToCatchAll);
    assert_eq!(plan.budget.max_unsent_windows, 8);
    let [entry] = plan.entries.as_slice() else {
        panic!("{:?}", plan.entries);
    };
    assert_eq!(entry.id, "e1");
    assert_eq!(
        entry.context_keys,
        [ContextKey::Named {
            name: "destination".into(),
            path: body(&["params", "arguments", "repo"]),
            field_type: "repository".into(),
        }]
    );
    assert_eq!(
        entry.measures,
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
    assert_eq!(
        plan.catch_all,
        [
            CatchAllKey::Host,
            CatchAllKey::PathPrefix { segments: 1 },
            CatchAllKey::Method,
            CatchAllKey::Operation,
        ]
    );
}

// What serde_json can't hold fails the condition, as in a bundle, not the plan.
#[test]
fn test_reads_a_condition_past_what_a_double_holds_as_invalid() {
    let vectors = vectors();
    let plan =
        vectors
            .plan
            .to_string()
            .replacen(r#""value":"api.github.com""#, r#""value":1e400"#, 1);

    let reading = parse_plan(plan.as_bytes(), &agent(&vectors)).unwrap();

    let PlanState::Valid(plan) = reading.state else {
        panic!("{:?}", reading.state);
    };
    assert!(matches!(
        plan.entries[0].conditions[0],
        Condition::Invalid(_)
    ));
}
