//! What the workspace's tests share: settings read as a binary reads them, a binary run as a
//! server, and a stub Rail Center for it.

use std::ffi::OsStr;
use std::io::{ErrorKind, Read};
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rail_runtime::{
    ConfigErrors, Level, ProtectedHosts, Refused, Settings, Startup, Variables,
    read_protected_hosts,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;

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

/// Each entry `reader` logs of `pairs`, as (name, value, level); `pairs` must be accepted.
pub fn describe<T>(
    pairs: &[(&str, &str)],
    reader: impl FnOnce(&mut Settings) -> Result<T, Refused>,
) -> Vec<(&'static str, String, Level)> {
    let (_, startup) = read(pairs, reader).unwrap();
    startup
        .entries
        .into_iter()
        .map(|entry| (entry.name, entry.value, entry.level))
        .collect()
}

/// The protected hosts `value` lists, which must be accepted.
pub fn protect(value: &str) -> ProtectedHosts {
    let pairs = [("RAIL_PROXY_PROTECTED_HOSTS", value)];
    read(&pairs, |settings| read_protected_hosts(settings, true))
        .unwrap()
        .0
}

/// A port nothing listens on, for the server to bind.
pub fn free_port() -> u16 {
    StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
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
pub struct Server {
    child: Child,
    // Its log so far, read as it's written, so a test can wait on a line.
    logs: Arc<Mutex<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
}

impl Server {
    /// `command` spawned, its log kept.
    pub fn spawn(mut command: Command) -> Self {
        let mut child = command
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&logs);
        let reader = std::thread::spawn(move || {
            let mut chunk = [0; 4096];
            loop {
                match stderr.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => kept.lock().unwrap().extend_from_slice(&chunk[..read]),
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(error) => panic!("can't read the server's log: {error}"),
                }
            }
        });
        Self {
            child,
            logs,
            reader: Some(reader),
        }
    }

    /// `command` spawned, once `serving` holds; it must hold within 10 s.
    pub fn serve(command: Command, serving: impl FnMut() -> bool) -> Self {
        let server = Self::spawn(command);
        poll(serving, "the server never served");
        server
    }

    /// Sends SIGTERM.
    pub fn terminate(&self) {
        let status = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// The exit code, or `None` if it's still running at the deadline.
    pub fn wait_for_exit(&mut self, deadline: Duration) -> Option<Option<i32>> {
        let until = Instant::now() + deadline;
        while Instant::now() < until {
            if let Some(status) = self.child.try_wait().unwrap() {
                return Some(status.code());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// Once it has logged `text`; it must within 10 s.
    pub fn wait_for_log(&self, text: &str) {
        poll(
            || String::from_utf8_lossy(&self.logs.lock().unwrap()).contains(text),
            "never logged",
        );
    }

    /// What it logged, once it has exited.
    pub fn read_logs(&mut self) -> String {
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
        String::from_utf8_lossy(&self.logs.lock().unwrap()).into_owned()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A report's request line and body.
pub type Posted = (String, Vec<u8>);

/// Rail Center, answering the bundle route with `bundle`, the key set with no keys, the capture
/// plan route with its plan, or a 204 without one, and taking every report; `seen` holds each
/// request line, `posted` each report's line and body.
pub struct RailCenter {
    address: SocketAddr,
    pub seen: Arc<Mutex<Vec<String>>>,
    pub posted: Arc<Mutex<Vec<Posted>>>,
    bundle_held: watch::Sender<bool>,
}

impl RailCenter {
    pub async fn start(bundle: &'static str) -> Self {
        Self::start_with(bundle, false, None).await
    }

    /// One that doesn't answer the bundle route until `release_bundle`.
    pub async fn start_holding_bundle(bundle: &'static str) -> Self {
        Self::start_with(bundle, true, None).await
    }

    /// One serving `plan` as the capture plan.
    pub async fn start_with_plan(plan: &'static str) -> Self {
        Self::start_with("{}", false, Some(plan)).await
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn release_bundle(&self) {
        self.bundle_held.send_replace(false);
    }

    async fn start_with(bundle: &'static str, held: bool, plan: Option<&'static str>) -> Self {
        let (bundle_held, released) = watch::channel(held);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let posted = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let reports = Arc::clone(&posted);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let log = Arc::clone(&log);
                let reports = Arc::clone(&reports);
                let mut released = released.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    while !head.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        if stream.read(&mut byte).await.unwrap_or(0) == 0 {
                            return;
                        }
                        head.push(byte[0]);
                    }
                    let head = String::from_utf8_lossy(&head).into_owned();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    let mut body = vec![0; length];
                    stream.read_exact(&mut body).await.unwrap();
                    let line = head.lines().next().unwrap().to_owned();
                    let (status, reply) = if line.starts_with("GET /v1/policy-bundle") {
                        let _ = released.wait_for(|held| !held).await;
                        ("200 OK", bundle)
                    } else if line.starts_with("GET /v1/jwks.json") {
                        ("200 OK", r#"{"keys": []}"#)
                    } else if line.starts_with("GET /v1/capture-plan") {
                        plan.map_or(("204 No Content", ""), |plan| ("200 OK", plan))
                    } else {
                        // A report, which Rail Center accepts; a ticket asked for gets the same.
                        if line.starts_with("POST ") {
                            reports.lock().unwrap().push((line.clone(), body.clone()));
                        }
                        ("202 Accepted", "{}")
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    // Seen once answered, so a test waiting on it sees the caller's answer too.
                    log.lock().unwrap().push(line);
                });
            }
        });
        Self {
            address,
            seen,
            posted,
            bundle_held,
        }
    }
}
