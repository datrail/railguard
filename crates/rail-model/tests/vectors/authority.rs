use rail_model::authority::parse_authority;
use serde::Deserialize;

const VECTORS: &str = include_str!("../../../../vectors/authority.json");

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    authority: String,
    host: String,
    port: Option<u16>,
}

#[test]
fn test_vectors() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());
    for case in vectors.cases {
        assert_eq!(
            parse_authority(&case.authority),
            Ok((case.host, case.port)),
            "{}",
            case.id
        );
    }
}
