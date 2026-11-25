use rail_model::policy::parse_bundle;
use serde::Deserialize;
use serde_json::value::RawValue;
use uuid::Uuid;

const VECTORS: &str = include_str!("../../../../vectors/policy/bundle.json");
#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    // Raw, since a priority may be past what a `Value` holds.
    bundle: Box<RawValue>,
    usable: bool,
    /// Usable cases only: the enabled policy ids, in evaluation order.
    chain: Option<Vec<Uuid>>,
    /// Usable cases only: the priorities of `chain`, in the same order.
    chain_priorities: Option<Vec<i64>>,
}

#[test]
fn test_vectors() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());

    let mut failures = Vec::new();
    for case in vectors.cases {
        let name = case.id;
        let result = parse_bundle(case.bundle.get());
        if case.usable == result.is_err() {
            failures.push(format!("{name}: {result:?}"));
        } else if let Ok(bundle) = result {
            let (chain, priorities) = bundle
                .policies
                .iter()
                .filter(|p| p.enabled)
                .map(|p| (p.id, p.priority))
                .unzip();
            if case.chain != Some(chain) || case.chain_priorities != Some(priorities) {
                failures.push(format!("{name}: wrong chain"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
