use std::time::{Duration, Instant};

use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_client::ExternalProcessorClient;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request;
use envoy_types::pb::envoy::service::ext_proc::v3::{HttpHeaders, ProcessingRequest};
use tokio_stream::StreamExt;
use tonic::transport::Endpoint;

use crate::common::{TempSocket, rail_envoy, serve, start_serving};

#[test]
fn with_nothing_open_it_stops_at_once() {
    let socket = TempSocket::new("idle");
    let mut server = start_serving(&socket.path);
    let started = Instant::now();

    server.terminate();

    assert_eq!(server.wait_for_exit(Duration::from_secs(2)), Some(Some(0)));
    assert!(started.elapsed() < Duration::from_secs(2));
    let logs = server.read_logs();
    assert!(
        logs.contains("shut down: every connection drained"),
        "{logs}"
    );
}

#[test]
fn with_rail_center_unreachable_it_serves_and_stops_at_once() {
    let socket = TempSocket::new("rail-center");
    let mut command = rail_envoy(&socket.path);
    command.envs([
        ("RAIL_DISABLED", "false"),
        ("RAIL_CENTER_URL", "http://127.0.0.1:1"),
        ("RAIL_ALLOW_HTTP", "true"),
        ("RAIL_HOST_ID", "host-1"),
        ("RAIL_SANDBOX_NAME", "sandbox-1"),
        ("RAIL_PROXY_PROTECTED_HOSTS", "mcp.example.com"),
    ]);
    let mut server = serve(command, &socket.path);
    let started = Instant::now();

    server.terminate();

    assert_eq!(server.wait_for_exit(Duration::from_secs(3)), Some(Some(0)));
    assert!(started.elapsed() < Duration::from_secs(3));
    let logs = server.read_logs();
    assert!(logs.contains("ticket refresh failed"), "{logs}");
    assert!(
        logs.contains("shut down: every connection drained"),
        "{logs}"
    );
    assert!(!logs.contains("panicked"), "{logs}");
}

// Envoy keeps a stream open as long as its request, so only the deadline ends it.
#[tokio::test(flavor = "multi_thread")]
async fn an_open_stream_is_cut_at_the_deadline() {
    let socket = TempSocket::new("open-stream");
    let mut server = tokio::task::block_in_place(|| start_serving(&socket.path));
    let channel = Endpoint::from_shared(format!("unix:{}", socket.path.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let headers = ProcessingRequest {
        request: Some(Request::RequestHeaders(HttpHeaders::default())),
        ..Default::default()
    };
    // The headers, then a request side that never closes.
    let requests = tokio_stream::iter([headers]).chain(tokio_stream::pending());
    let mut replies = ExternalProcessorClient::new(channel)
        .process(requests)
        .await
        .unwrap()
        .into_inner();
    assert!(replies.message().await.unwrap().is_some());
    let started = Instant::now();

    server.terminate();

    let exit = tokio::task::block_in_place(|| server.wait_for_exit(Duration::from_secs(7)));
    let elapsed = started.elapsed();
    assert_eq!(exit, Some(Some(0)));
    assert!(elapsed >= Duration::from_secs(5), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(6500), "{elapsed:?}");
    let logs = server.read_logs();
    assert!(logs.contains("shut down at the deadline"), "{logs}");
}
