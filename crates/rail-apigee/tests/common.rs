//! The real binary, on a port of its own, and a Rail Center it can fetch from.

use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use apigee_callout_proto::apigee::external_callout_service_client::ExternalCalloutServiceClient;
use rail_test_support::{Server, isolate};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;
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
    let mut command = isolate(env!("CARGO_BIN_EXE_rail-apigee"));
    command
        .env("RAIL_DISABLED", "true")
        .env("RAIL_GATEWAY_PORT", port.to_string());
    command
}

pub fn probe(port: u16, arguments: &[&str]) -> Output {
    rail_apigee(port)
        .arg("probe")
        .args(arguments)
        .output()
        .unwrap()
}

/// `command` spawned, once the liveness probe on `port` passes.
pub fn serve(command: Command, port: u16) -> Server {
    Server::serve(command, || probe(port, &[]).status.success())
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
    address: SocketAddr,
    pub seen: Arc<Mutex<Vec<String>>>,
    bundle_held: watch::Sender<bool>,
}

impl RailCenter {
    pub async fn start(bundle: &'static str) -> Self {
        Self::start_with(bundle, false).await
    }

    /// One that doesn't answer the bundle route until `release_bundle`.
    pub async fn start_holding_bundle(bundle: &'static str) -> Self {
        Self::start_with(bundle, true).await
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn release_bundle(&self) {
        self.bundle_held.send_replace(false);
    }

    async fn start_with(bundle: &'static str, held: bool) -> Self {
        let (bundle_held, released) = watch::channel(held);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let log = Arc::clone(&log);
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
                    let reply = if line.starts_with("GET /v1/policy-bundle") {
                        let _ = released.wait_for(|held| !held).await;
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
        Self {
            address,
            seen,
            bundle_held,
        }
    }
}
