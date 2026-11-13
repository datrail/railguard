//! The real binary, on a port of its own, and a Rail Center it can fetch from.

use std::process::{Command, Output};

use apigee_callout_proto::apigee::external_callout_service_client::ExternalCalloutServiceClient;
pub use rail_test_support::{RailCenter, free_port};
use rail_test_support::{Server, isolate};
use tonic::transport::{Channel, Endpoint};

/// The binary serving as the gateway on `port`, with Rail Center disabled.
pub fn rail_apigee(port: u16) -> Command {
    let mut command = with_settings(port);
    command.arg("gateway");
    command
}

/// The binary with no arguments; only what's set here, so the caller's own RAIL_* variables
/// can't change a run.
pub fn with_settings(port: u16) -> Command {
    let mut command = isolate(env!("CARGO_BIN_EXE_rail-apigee"));
    command
        .env("RAIL_DISABLED", "true")
        // The tests read text lines; JSON is asked for where it's tested.
        .env("RAIL_LOG_FORMAT", "text")
        .env("RAIL_CALLOUT_PORT", port.to_string());
    command
}

pub fn probe(port: u16, arguments: &[&str]) -> Output {
    with_settings(port)
        .arg("probe")
        .args(arguments)
        .output()
        .unwrap()
}

/// `command` spawned, once the liveness probe on `port` passes.
pub fn serve(command: Command, port: u16) -> Server {
    Server::serve(command, || probe(port, &[]).status.success())
}

/// A client for the server on `port`, sending and receiving up to 4 MiB.
pub async fn client(port: u16) -> ExternalCalloutServiceClient<Channel> {
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    ExternalCalloutServiceClient::new(channel)
        .max_decoding_message_size(4 << 20)
        .max_encoding_message_size(4 << 20)
}
