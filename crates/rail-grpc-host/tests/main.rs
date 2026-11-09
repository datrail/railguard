use std::process::ExitCode;

use rail_grpc_host::{Address, probe};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic_health::ServingStatus;

// Over TCP, as rail-apigee serves, each service asked by its name.
#[tokio::test(flavor = "multi_thread")]
async fn the_probe_asks_a_tcp_server_for_the_service_named() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = Address::Tcp(listener.local_addr().unwrap());
    let (health, health_service) = tonic_health::server::health_reporter();
    health.set_service_status("", ServingStatus::Serving).await;
    health
        .set_service_status("test.Ready", ServingStatus::NotServing)
        .await;
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(health_service)
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );

    assert_eq!(probe(&address, "").await, ExitCode::SUCCESS);
    assert_eq!(probe(&address, "test.Ready").await, ExitCode::FAILURE);
}
