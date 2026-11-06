//! The local Rail Center the tests talk to, and settings read as a binary reads them.
// Shared by both test binaries, and each uses only part of it.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rail_runtime::{
    ConfigErrors, Level, ProtectedHosts, RailCenterClient, RailCenterSettings, Refused, Settings,
    Startup, Variables, read_protected_hosts, read_rail_center,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const TOKEN: &str = "rc_svc_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

// What the test server does with the nth connection.
pub enum Reply {
    Raw(Vec<u8>),
    // Writes the head, then one body byte per interval, for ever.
    Dribble(Vec<u8>, Duration),
    // Closes without answering, as a pooled connection the peer dropped.
    Close,
    // Reads the request and never answers, holding the connection open.
    Silent,
    // Answers every request on the connection with these bytes, keeping it open.
    KeepAlive(Vec<u8>),
}

pub struct Server {
    pub url: String,
    pub requests: Arc<Mutex<Vec<String>>>,
}

pub async fn serve(reply: impl Fn(usize) -> Reply + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    tokio::spawn(async move {
        let mut count = 0;
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let seen = Arc::clone(&seen);
            let reply = reply(count);
            count += 1;
            tokio::spawn(async move { answer(stream, reply, &seen).await });
        }
    });
    Server { url, requests }
}

pub async fn answer(mut stream: TcpStream, reply: Reply, seen: &Mutex<Vec<String>>) {
    if !read_head(&mut stream, seen).await {
        return;
    }
    match reply {
        Reply::Raw(bytes) => {
            let _ = stream.write_all(&bytes).await;
            let _ = stream.shutdown().await;
        }
        Reply::Dribble(head, interval) => {
            let _ = stream.write_all(&head).await;
            while stream.write_all(b"x").await.is_ok() {
                tokio::time::sleep(interval).await;
            }
        }
        Reply::Close => {}
        Reply::Silent => {
            std::future::pending::<()>().await;
            drop(stream);
        }
        Reply::KeepAlive(bytes) => {
            while stream.write_all(&bytes).await.is_ok() && read_head(&mut stream, seen).await {}
        }
    }
}

// Reads one request's head into `seen`; false if the peer closed first.
async fn read_head(stream: &mut TcpStream, seen: &Mutex<Vec<String>>) -> bool {
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).await.unwrap_or(0) == 0 {
            return false;
        }
        head.push(byte[0]);
    }
    seen.lock().unwrap().push(String::from_utf8(head).unwrap());
    true
}

pub fn response(status: &str, headers: &[&str], body: &[u8]) -> Vec<u8> {
    let mut bytes = format!("HTTP/1.1 {status}\r\nconnection: close\r\n").into_bytes();
    for header in headers {
        bytes.extend_from_slice(format!("{header}\r\n").as_bytes());
    }
    bytes.extend_from_slice(format!("content-length: {}\r\n\r\n", body.len()).as_bytes());
    bytes.extend_from_slice(body);
    bytes
}

pub fn settings(url: &str, token: Option<&str>) -> RailCenterSettings {
    let mut pairs = vec![("RAIL_CENTER_URL", url), ("RAIL_ALLOW_HTTP", "true")];
    if let Some(token) = token {
        pairs.extend([("RAIL_AUTH_MODE", "bearer"), ("RAIL_AUTH_TOKEN", token)]);
    }
    read(&pairs, |settings| read_rail_center(settings, true))
        .unwrap()
        .unwrap()
}

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

/// What `reader` reads of `pairs`, or every refusal.
pub fn read<T>(
    pairs: &[(&str, &str)],
    reader: impl FnOnce(&mut Settings) -> Result<T, Refused>,
) -> Result<T, Vec<String>> {
    let variables: Variables = pairs.iter().copied().collect();
    read_from(&variables, reader).map(|(read, _)| read)
}

/// Each entry `reader` logs of `pairs`, as (name, value, level); `pairs` must be accepted.
pub fn describe<T>(
    pairs: &[(&str, &str)],
    reader: impl FnOnce(&mut Settings) -> Result<T, Refused>,
) -> Vec<(&'static str, String, Level)> {
    let variables: Variables = pairs.iter().copied().collect();
    let (_, startup) = read_from(&variables, reader).unwrap();
    startup
        .entries
        .into_iter()
        .map(|entry| (entry.name, entry.value, entry.level))
        .collect()
}

/// The protected hosts `value` lists, which must be accepted.
pub fn protect(value: &str) -> ProtectedHosts {
    read(&[("RAIL_PROXY_PROTECTED_HOSTS", value)], |settings| {
        read_protected_hosts(settings, true)
    })
    .unwrap()
}

pub fn client(url: &str) -> RailCenterClient {
    RailCenterClient::new(&settings(url, Some(TOKEN))).unwrap()
}
