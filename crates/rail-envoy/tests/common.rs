//! The real binary, run on a socket of its own.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use rail_test_support::{Server, isolate};

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
