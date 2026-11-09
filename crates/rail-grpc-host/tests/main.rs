use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rail_grpc_host::{Address, ask_health};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic_health::ServingStatus;

// A socket path in a directory of its own, removed after.
struct TempSocket {
    dir: PathBuf,
    path: PathBuf,
}

impl TempSocket {
    fn new(test: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("rail-grpc-host-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("grpc.sock");
        Self { dir, path }
    }
}

impl Drop for TempSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

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

    assert_eq!(ask_health(&address, "").await, Ok("serving".to_owned()));
    assert_eq!(
        ask_health(&address, "test.Ready").await,
        Err("not serving: NOT_SERVING".to_owned())
    );
}

// The innermost cause, which over TCP tonic wraps twice.
#[tokio::test]
async fn the_probe_fails_with_nothing_listening() {
    let socket = TempSocket::new("nothing");
    let unused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = Address::Tcp(unused.local_addr().unwrap());
    drop(unused);

    assert_eq!(
        ask_health(&Address::Unix(socket.path.clone()), "").await,
        Err(format!(
            "can't check health on {}: can't connect: No such file or directory (os error 2)",
            socket.path.display()
        ))
    );
    assert_eq!(
        ask_health(&port, "").await,
        Err(format!(
            "can't check health on {port}: can't connect: Connection refused (os error 111)"
        ))
    );
}

// The kernel completes the connection from the backlog, so only the request can tell.
#[tokio::test]
async fn the_probe_fails_on_a_socket_nothing_answers() {
    let socket = TempSocket::new("hung");
    let _listener = StdUnixListener::bind(&socket.path).unwrap();
    let started = Instant::now();

    let answer = ask_health(&Address::Unix(socket.path.clone()), "").await;

    assert_eq!(
        answer,
        Err(format!("no answer on {} within 5s", socket.path.display()))
    );
    assert!(started.elapsed() < Duration::from_secs(8));
}
