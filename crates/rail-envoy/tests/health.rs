use std::os::unix::net::UnixListener as StdUnixListener;
use std::time::{Duration, Instant};

use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic_health::ServingStatus;

use crate::common::{Server, TempSocket, probe};

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn the_probe_passes_on_a_serving_server() {
    let socket = TempSocket::new("serving");
    let _server = Server::start_serving(&socket.path);

    let output = probe(&socket.path);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output), "serving\n");
}

#[test]
fn the_probe_fails_with_nothing_on_the_socket() {
    let socket = TempSocket::new("nothing");

    let output = probe(&socket.path);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).starts_with(&format!(
            "can't check health on {}: ",
            socket.path.display()
        )),
        "{output:?}"
    );
}

// The kernel completes the connection from the backlog, so only the request can tell.
#[test]
fn the_probe_fails_on_a_socket_nothing_answers() {
    let socket = TempSocket::new("hung");
    let _listener = StdUnixListener::bind(&socket.path).unwrap();
    let started = Instant::now();

    let output = probe(&socket.path);

    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).ends_with("within 5s\n"), "{output:?}");
    assert!(started.elapsed() < Duration::from_secs(8));
}

#[tokio::test]
async fn the_probe_fails_on_a_server_not_serving() {
    let socket = TempSocket::new("not-serving");
    let (health, service) = tonic_health::server::health_reporter();
    health
        .set_service_status("", ServingStatus::NotServing)
        .await;
    let incoming = UnixListenerStream::new(UnixListener::bind(&socket.path).unwrap());
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(incoming),
    );
    let path = socket.path.clone();

    let output = tokio::task::spawn_blocking(move || probe(&path))
        .await
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "not serving: NOT_SERVING\n");
}
