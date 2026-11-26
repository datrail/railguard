//! Calls as a host sends them, for the gateway's tests.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use rail_runtime::{Answer, BodyEnding, Gateway};

#[derive(Clone)]
pub struct Call {
    pub path: String,
    pub method: String,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

// What `gateway` does with `call`, gathered as its host gathers it, the body sent whole; and the
// body back.
pub async fn handle(gateway: &Gateway, call: Call) -> (Answer, Vec<u8>) {
    handle_ended(gateway, call, BodyEnding::Whole).await
}

// As `handle`, its body ending as `ending` tells.
pub async fn handle_ended(gateway: &Gateway, call: Call, ending: BodyEnding) -> (Answer, Vec<u8>) {
    let line = [(":path", &call.path), (":method", &call.method)];
    let lines = line.into_iter().chain(
        call.headers
            .iter()
            .map(|(name, value)| (name.as_str(), value)),
    );
    let mut gatherer = gateway.gather(lines.map(|(name, value)| (name, value.as_bytes())));
    gatherer.adopt(call.body.into_bytes());
    gateway.handle(gatherer.finish(ending)).await
}
