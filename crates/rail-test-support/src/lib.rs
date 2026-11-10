//! What the workspace's tests share: settings read as a binary reads them, and a binary run as a
//! server.

use std::ffi::OsStr;
use std::io::Read;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use rail_runtime::{
    ConfigErrors, ProtectedHosts, Refused, Settings, Startup, Variables, read_protected_hosts,
};

/// What `reader` reads of `variables`, and what that logs, or every refusal.
pub fn read_from<T>(
    variables: &Variables,
    reader: impl FnOnce(&mut Settings) -> Result<T, Refused>,
) -> Result<(T, Startup), Vec<String>> {
    let mut settings = Settings::new(variables);
    let read = reader(&mut settings);
    settings
        .finish("RAIL_", read)
        .map_err(|ConfigErrors(errors)| errors)
}

/// What `reader` reads of `pairs`, and what that logs, or every refusal.
pub fn read<T>(
    pairs: &[(&str, &str)],
    reader: impl FnOnce(&mut Settings) -> Result<T, Refused>,
) -> Result<(T, Startup), Vec<String>> {
    let variables: Variables = pairs.iter().copied().collect();
    read_from(&variables, reader)
}

/// The protected hosts `value` lists, which must be accepted.
pub fn protect(value: &str) -> ProtectedHosts {
    let pairs = [("RAIL_PROXY_PROTECTED_HOSTS", value)];
    read(&pairs, |settings| read_protected_hosts(settings, true))
        .unwrap()
        .0
}

/// `program` with an empty environment, so the caller's own RAIL_* variables can't change a run.
pub fn isolate(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    // Under `cargo llvm-cov`, so the binary's coverage is counted, not left in the crate.
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command
}

/// What `output` printed on stdout.
pub fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// Polls `condition` for up to 10 s.
pub fn wait_until(condition: impl FnMut() -> bool) {
    poll(condition, "never happened");
}

fn poll(mut condition: impl FnMut() -> bool, failure: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "{failure}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The server, killed if the test ends before it exits.
pub struct Server(Child);

impl Server {
    /// `command` spawned, its log kept.
    pub fn spawn(mut command: Command) -> Self {
        let child = command
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self(child)
    }

    /// `command` spawned, once `serving` holds; it must within 10 s.
    pub fn serve(command: Command, serving: impl FnMut() -> bool) -> Self {
        let server = Self::spawn(command);
        poll(serving, "the server never served");
        server
    }

    /// Sends SIGTERM.
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
