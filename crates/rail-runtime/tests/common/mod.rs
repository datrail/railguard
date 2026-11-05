//! The local Rail Center the tests talk to, and the entries a setting describes.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rail_runtime::{
    Described, Level, RailCenterClient, RailCenterSettings, Settings, Variables, read_rail_center,
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
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).await.unwrap_or(0) == 0 {
            return;
        }
        head.push(byte[0]);
    }
    seen.lock().unwrap().push(String::from_utf8(head).unwrap());
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
    }
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
    let variables: Variables = pairs.into_iter().collect();
    let mut reader = Settings::new(&variables);
    let read = read_rail_center(&mut reader, true);
    reader.finish().unwrap();
    read.expect("read_rail_center returns the settings when it refuses nothing")
}

// Each entry `write` describes, as (name, value, level).
pub fn describe(write: impl FnOnce(&mut Described)) -> Vec<(&'static str, String, Level)> {
    let mut described = Described::default();
    write(&mut described);
    described
        .entries()
        .iter()
        .map(|entry| (entry.name, entry.value.clone(), entry.level))
        .collect()
}

pub fn client(url: &str) -> RailCenterClient {
    RailCenterClient::new(&settings(url, Some(TOKEN))).unwrap()
}
