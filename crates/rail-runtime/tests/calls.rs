//! Calls as a host sends them, for the gateway's tests.

use rail_runtime::{Answer, BodyEnding, Gateway, RequestGatherer};

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
// body back, as it came for a call it doesn't judge, which every host forwards.
pub async fn handle(gateway: &Gateway, call: Call) -> (Answer, Vec<u8>) {
    let line = [(":path", &call.path), (":method", &call.method)];
    let lines = line.into_iter().chain(
        call.headers
            .iter()
            .map(|(name, value)| (name.as_str(), value)),
    );
    let gathered = gateway.gather(lines.map(|(name, value)| (name, value.as_bytes())));
    let Some(mut gatherer) = gathered.judged() else {
        return (Answer::Forward, call.body.into_bytes());
    };
    gatherer.adopt(call.body.into_bytes());
    gateway.handle(gatherer, BodyEnding::Whole).await
}

// The gatherer of a `POST` to `/mcp`, which `gateway` judges.
pub fn judged_post(gateway: &Gateway) -> RequestGatherer {
    judged(gateway, [(":method", "POST"), (":path", "/mcp")])
}

// The gatherer of the call whose header lines are `lines`, a `POST`, which `gateway` judges.
pub fn judged<'a>(
    gateway: &Gateway,
    lines: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> RequestGatherer {
    gateway.gather(lines).judged().expect("a POST is judged")
}
