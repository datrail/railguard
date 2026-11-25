use rail_model::request_line::RequestLine;
use serde::Deserialize;

const VECTORS: &str = include_str!("../../../../vectors/redaction.json");

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    path: String,
    redacted: String,
}

fn cases() -> Vec<Case> {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    assert!(!vectors.cases.is_empty());
    vectors.cases
}

fn redact(path: &str) -> String {
    let line = RequestLine::read(b"mcp.example.com", b"POST", path.as_bytes());
    line.redacted_path().to_owned()
}

#[test]
fn test_vectors() {
    for case in cases() {
        assert_eq!(redact(&case.path), case.redacted, "{}", case.id);
    }
}

#[test]
fn test_redacting_twice_is_redacting_once() {
    for case in cases() {
        assert_eq!(redact(&case.redacted), case.redacted, "{}", case.id);
    }
}
