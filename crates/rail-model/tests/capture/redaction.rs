use rail_model::capture::CallLine;
use serde::Deserialize;

const VECTORS: &str = include_str!("../../../../vectors/capture/redaction.json");

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    path: String,
    redacted: String,
}

fn cases() -> Vec<Case> {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());
    vectors.cases
}

fn redact(path: &str) -> String {
    let line = CallLine::read(b"mcp.example.com", b"POST", path.as_bytes());
    line.request_line().redacted_path.clone()
}

#[test]
fn test_vectors() {
    for case in cases() {
        assert_eq!(redact(&case.path), case.redacted, "{}", case.name);
    }
}

#[test]
fn test_redacting_twice_is_redacting_once() {
    for case in cases() {
        assert_eq!(redact(&case.redacted), case.redacted, "{}", case.name);
    }
}
