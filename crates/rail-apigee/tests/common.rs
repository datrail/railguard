//! The real binary, on a port of its own, and a Rail Center it can fetch from.

use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apigee_callout_proto::apigee::external_callout_service_client::ExternalCalloutServiceClient;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tonic::transport::{Channel, Endpoint};

/// A port nothing listens on, for the server to bind.
pub fn free_port() -> u16 {
    StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

// Only what's set here, so the caller's own RAIL_* variables can't change a run.
pub fn rail_apigee(port: u16) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rail-apigee"));
    command
        .env_clear()
        .env("RAIL_DISABLED", "true")
        .env("RAIL_GATEWAY_PORT", port.to_string());
    // Under `cargo llvm-cov`, so the binary's coverage is counted, not left in the crate.
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command
}

pub fn probe(port: u16, arguments: &[&str]) -> Output {
    rail_apigee(port)
        .arg("probe")
        .args(arguments)
        .output()
        .unwrap()
}

/// The server, killed if the test ends before it exits.
pub struct Server(Child);

impl Server {
    /// `command` spawned, its log kept, once the liveness probe on `port` passes.
    pub fn serve(mut command: Command, port: u16) -> Self {
        let child = command
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let server = Self(child);
        wait_until(|| probe(port, &[]).status.success());
        server
    }

    pub fn terminate(&self) {
        let status = Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// The exit code, and what it logged; it must exit within 10 s.
    pub fn wait_for_exit(mut self) -> (Option<i32>, String) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "the server never exited");
            std::thread::sleep(Duration::from_millis(20));
        };
        let mut logs = String::new();
        self.0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut logs)
            .unwrap();
        (status.code(), logs)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Polls `condition` for up to 10 s.
pub fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A client for the server on `port`, sending and receiving up to 4 MiB.
pub async fn client(port: u16) -> ExternalCalloutServiceClient<Channel> {
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    ExternalCalloutServiceClient::new(channel)
        .max_decoding_message_size(4 << 20)
        .max_encoding_message_size(4 << 20)
}

/// Rail Center, answering the bundle route with `bundle`, the key set with no keys, and taking
/// every denial report; `seen` holds each request line.
pub struct RailCenter {
    pub address: SocketAddr,
    pub seen: Arc<Mutex<Vec<String>>>,
}

impl RailCenter {
    pub async fn start(bundle: &'static str) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let log = Arc::clone(&log);
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
                    let reply = if line.starts_with("GET /v1/policy-bundle") {
                        bundle
                    } else if line.starts_with("GET /v1/jwks.json") {
                        r#"{"keys": []}"#
                    } else {
                        "{}"
                    };
                    log.lock().unwrap().push(line);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Self { address, seen }
    }
}
