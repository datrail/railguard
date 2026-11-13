mod command;
mod health;

use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use rail_grpc_host::{Address, ask_health, probe};

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
    assert_eq!(probe(&port, false).await, ExitCode::FAILURE);
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
