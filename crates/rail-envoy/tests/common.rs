//! The real binary, run on a socket of its own.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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

// Only what's set here, so the caller's own RAIL_* variables can't change a run.
pub fn rail_envoy(socket: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rail-envoy"));
    command
        .env_clear()
        .env("RAIL_DISABLED", "true")
        .env("RAIL_EXT_SOCKET", socket);
    // Under `cargo llvm-cov`, so the binary's coverage is counted, not left in the crate.
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command
}

/// The server, killed if the test ends before it exits.
pub struct Server(pub Child);

impl Server {
    pub fn start(socket: &Path) -> Self {
        let child = rail_envoy(socket)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self(child)
    }

    /// Waits for the probe to pass, so the server is serving.
    pub fn start_serving(socket: &Path) -> Self {
        let server = Self::start(socket);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !probe(socket).status.success() {
            assert!(
                Instant::now() < deadline,
                "the server never answered SERVING"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        server
    }

    pub fn terminate(&self) {
        let status = Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// The exit code, or `None` if it's still running at the deadline.
    pub fn wait_for_exit(&mut self, deadline: Duration) -> Option<Option<i32>> {
        let until = Instant::now() + deadline;
        while Instant::now() < until {
            if let Some(status) = self.0.try_wait().unwrap() {
                return Some(status.code());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// What it logged, once it has exited.
    pub fn read_logs(&mut self) -> String {
        let mut logs = String::new();
        let mut stderr = self.0.stderr.take().unwrap();
        stderr.read_to_string(&mut logs).unwrap();
        logs
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn probe(socket: &Path) -> Output {
    rail_envoy(socket).arg("probe").output().unwrap()
}
