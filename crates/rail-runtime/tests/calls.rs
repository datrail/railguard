//! Calls as a host sends them, for the gateway's tests.

use rail_runtime::{Answer, BodyEnding, Gateway, Gathering, RequestGatherer};

use crate::scanning::SCANNED;

#[derive(Clone)]
pub struct Call {
    pub path: String,
    pub method: String,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

// A call to `/mcp` reading a resource, its ticket `x_rail`, if any.
pub fn call(method: &str, x_rail: Option<&str>) -> Call {
    Call {
        path: "/mcp".into(),
        method: method.into(),
        body: r#"{"method": "resources/read"}"#.into(),
        headers: x_rail
            .map(|ticket| ("x-rail".into(), ticket.into()))
            .into_iter()
            .collect(),
    }
}

// A call `scanning`'s policies are slow to judge.
pub fn scanned_call() -> Call {
    Call {
        headers: vec![("x-a".into(), "a".repeat(SCANNED))],
        ..call("POST", None)
    }
}

// What `gateway` does with `call`, gathered as its host gathers it, the body sent whole; and the
// body back, as it came for a call it doesn't judge.
pub async fn handle(gateway: &Gateway, call: Call) -> (Answer, Vec<u8>) {
    let line = [(":path", &call.path), (":method", &call.method)];
    let lines = line.into_iter().chain(
        call.headers
            .iter()
            .map(|(name, value)| (name.as_str(), value)),
    );
    match gateway.gather(lines.map(|(name, value)| (name, value.as_bytes()))) {
        Gathering::Unjudged => (Answer::Forward, call.body.into_bytes()),
        Gathering::Judged(mut gatherer) => {
            gatherer.adopt(call.body.into_bytes());
            gateway.handle(gatherer, BodyEnding::Whole).await
        }
    }
}

// The gatherer of a `POST` to `/mcp`, which `gateway` judges.
pub fn judged_post(gateway: &Gateway) -> RequestGatherer {
    match gateway.gather([(":method", "POST"), (":path", "/mcp")]) {
        Gathering::Judged(gatherer) => gatherer,
        Gathering::Unjudged => panic!("a POST is judged"),
    }
}
