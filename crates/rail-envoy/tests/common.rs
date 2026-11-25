//! The real binary, run on a socket of its own.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessorServer;
use rail_envoy::Callouts;
use rail_test_support::{Server, isolate};
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Channel, Endpoint};

// Every test runs in one process, so the pid alone doesn't keep their directories apart.
static SOCKETS: AtomicUsize = AtomicUsize::new(0);

/// A directory of its own for one test's socket, removed after.
pub struct TempSocket {
    dir: PathBuf,
    pub path: PathBuf,
}

impl TempSocket {
    pub fn new(test: &str) -> Self {
        let count = SOCKETS.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("rail-envoy-{}-{count}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let path = dir.join("ext.sock");
        Self { dir, path }
    }
}

impl Drop for TempSocket {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// `callouts`' ext_proc served in-process on a socket of its own, and a channel to it.
pub async fn serve_ext_proc(test: &str, callouts: Callouts) -> (TempSocket, Channel) {
    let socket = TempSocket::new(test);
    let listener = UnixListener::bind(&socket.path).unwrap();
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(ExternalProcessorServer::new(callouts))
            .serve_with_incoming(UnixListenerStream::new(listener)),
    );
    let channel = Endpoint::from_shared(format!("unix:{}", socket.path.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    (socket, channel)
}

/// The binary serving as the proxy on `socket`, with Rail Center disabled.
pub fn rail_envoy(socket: &Path) -> Command {
    let mut command = with_settings(socket);
    command.arg("proxy");
    command
}

/// The binary with no arguments; only what's set here, so the caller's own RAIL_* variables
/// can't change a run.
pub fn with_settings(socket: &Path) -> Command {
    let mut command = isolate(env!("CARGO_BIN_EXE_rail-envoy"));
    command
        .env("RAIL_DISABLED", "true")
        // The tests read text lines; JSON is asked for where it's tested.
        .env("RAIL_LOG_FORMAT", "text")
        .env("RAIL_EXT_SOCKET", socket);
    command
}

/// The server on `socket`, with Rail Center disabled, once it's serving.
pub fn start_serving(socket: &Path) -> Server {
    serve(rail_envoy(socket), socket)
}

/// `command` spawned, once the probe on `socket` passes.
pub fn serve(command: Command, socket: &Path) -> Server {
    Server::serve(command, || probe(socket).status.success())
}

pub fn probe(socket: &Path) -> Output {
    with_settings(socket).arg("probe").output().unwrap()
}

pub fn probe_ready(socket: &Path) -> Output {
    with_settings(socket)
        .args(["probe", "--ready"])
        .output()
        .unwrap()
}
