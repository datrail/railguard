use std::io::Write as _;
use std::time::Duration;

use flate2::Compression;
use flate2::write::GzEncoder;
use rail_runtime::{FetchError, Limits, RailCenterClient, RailCenterSettings};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use crate::common::{Reply, TOKEN, client, response, serve, settings};

const LIMITS: Limits = Limits {
    deadline: Duration::from_secs(10),
    silence: Duration::from_secs(5),
    max_bytes: 1024,
};

// A body with no declared length, so only the running count can bound it.
fn unsized_response(headers: &[&str], body: &[u8]) -> Vec<u8> {
    let mut bytes = b"HTTP/1.1 200 OK\r\nconnection: close\r\n".to_vec();
    for header in headers {
        bytes.extend_from_slice(format!("{header}\r\n").as_bytes());
    }
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(body);
    bytes
}

fn gzip(body: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(body).unwrap();
    encoder.finish().unwrap()
}

#[tokio::test]
async fn a_get_reaches_the_route_under_the_base_with_the_credential() {
    let server = serve(|_| Reply::Raw(response("200 OK", &[], b"{}"))).await;

    let fetched = client(&format!("{}/rc/", server.url))
        .get(
            "/v1/tickets",
            &[("host_id", "h 1"), ("sandbox_name", "s&t")],
            LIMITS,
        )
        .await
        .unwrap();

    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.body, b"{}");
    let requests = server.requests.lock().unwrap();
    let request = requests[0].to_ascii_lowercase();
    assert!(
        request.starts_with("get /rc/v1/tickets?host_id=h+1&sandbox_name=s%26t http/1.1\r\n"),
        "{request}"
    );
    assert!(request.contains(&format!(
        "\r\nauthorization: bearer {}\r\n",
        TOKEN.to_ascii_lowercase()
    )));
    assert!(request.contains("\r\naccept: application/json\r\n"));
    assert!(request.contains("\r\naccept-encoding: gzip, identity\r\n"));
    assert!(request.contains("\r\ncache-control: no-cache\r\n"));
}

#[tokio::test]
async fn no_credential_sends_no_authorization_header() {
    let server = serve(|_| Reply::Raw(response("200 OK", &[], b""))).await;
    let client = RailCenterClient::new(&settings(&server.url, None)).unwrap();

    client.get("/v1/policy-bundle", &[], LIMITS).await.unwrap();

    let requests = server.requests.lock().unwrap();
    assert!(!requests[0].to_ascii_lowercase().contains("authorization"));
}

#[tokio::test]
async fn a_redirect_is_answered_not_followed() {
    let elsewhere = serve(|_| Reply::Raw(response("200 OK", &[], b""))).await;
    let location = format!("location: {}/v1/tickets", elsewhere.url);
    let server = serve(move |_| Reply::Raw(response("302 Found", &[&location], b""))).await;

    let fetched = client(&server.url)
        .get("/v1/tickets", &[], LIMITS)
        .await
        .unwrap();

    assert_eq!(fetched.status, 302);
    assert!(elsewhere.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_declared_length_past_the_cap_is_refused_unread() {
    // The body never comes, so only the declared length can refuse it in time.
    let server = serve(|_| {
        Reply::Dribble(
            b"HTTP/1.1 200 OK\r\ncontent-length: 1025\r\n\r\n".to_vec(),
            Duration::from_secs(3600),
        )
    })
    .await;
    let limits = Limits {
        deadline: Duration::from_millis(200),
        ..LIMITS
    };

    let error = client(&server.url)
        .get("/v1/x", &[], limits)
        .await
        .unwrap_err();

    assert!(
        matches!(error, FetchError::TooLarge { max_bytes: 1024 }),
        "{error:?}"
    );
}

#[tokio::test]
async fn an_unsized_body_past_the_cap_is_refused_while_streaming() {
    let server = serve(|_| Reply::Raw(unsized_response(&[], &[b'x'; 1025]))).await;

    let error = client(&server.url)
        .get("/v1/x", &[], LIMITS)
        .await
        .unwrap_err();

    assert!(matches!(error, FetchError::TooLarge { .. }), "{error:?}");
}

#[tokio::test]
async fn a_body_at_the_cap_is_read() {
    let server = serve(|_| Reply::Raw(unsized_response(&[], &[b'x'; 1024]))).await;

    let fetched = client(&server.url).get("/v1/x", &[], LIMITS).await.unwrap();

    assert_eq!(fetched.body.len(), 1024);
}

#[tokio::test]
async fn a_gzip_body_is_decoded() {
    let server = serve(|_| {
        Reply::Raw(response(
            "200 OK",
            &["content-encoding: gzip"],
            &gzip(b"{\"a\":1}"),
        ))
    })
    .await;

    let fetched = client(&server.url).get("/v1/x", &[], LIMITS).await.unwrap();

    assert_eq!(fetched.body, b"{\"a\":1}");
}

#[tokio::test]
async fn a_gzip_bomb_is_refused_by_its_decoded_size() {
    let bomb = gzip(&[0; 64 << 10]);
    assert!(bomb.len() < LIMITS.max_bytes);
    let server =
        serve(move |_| Reply::Raw(response("200 OK", &["content-encoding: gzip"], &bomb))).await;

    let error = client(&server.url)
        .get("/v1/x", &[], LIMITS)
        .await
        .unwrap_err();

    assert!(matches!(error, FetchError::TooLarge { .. }), "{error:?}");
}

#[tokio::test]
async fn an_encoding_not_offered_or_behind_gzip_is_refused() {
    let cases: [(&[&str], &str); 6] = [
        (&["content-encoding: br"], "br"),
        (
            &["content-encoding: identity", "content-encoding: br"],
            "br",
        ),
        (&["content-encoding: gzip", "content-encoding: br"], "br"),
        (&["content-encoding: gzip, br"], "br"),
        (
            &["content-encoding: gzip", "content-encoding: gzip"],
            "gzip",
        ),
        (&["content-encoding: gzip, gzip"], "gzip"),
    ];
    for (headers, named) in cases {
        let server = serve(move |_| Reply::Raw(response("200 OK", headers, &gzip(b"{}")))).await;

        let error = client(&server.url)
            .get("/v1/x", &[], LIMITS)
            .await
            .unwrap_err();

        assert!(
            matches!(&error, FetchError::Encoding(encoding) if encoding.contains(named)),
            "{headers:?}: {error:?}"
        );
        assert!(
            error
                .to_string()
                .starts_with("the response's Content-Encoding is "),
            "{error}"
        );
    }
}

#[tokio::test]
async fn gzip_that_does_not_decode_is_refused() {
    let whole = gzip(b"{\"a\":1}");
    for body in [b"not gzip".to_vec(), whole[..whole.len() - 4].to_vec()] {
        let server =
            serve(move |_| Reply::Raw(response("200 OK", &["content-encoding: gzip"], &body)))
                .await;

        let error = client(&server.url)
            .get("/v1/x", &[], LIMITS)
            .await
            .unwrap_err();

        assert!(matches!(&error, FetchError::BadGzip(_)), "{error:?}");
        assert!(
            error
                .to_string()
                .starts_with("the response's gzip doesn't decode: "),
            "{error}"
        );
    }
}

#[tokio::test]
async fn identity_is_read_however_it_is_spelled() {
    let server =
        serve(|_| Reply::Raw(response("200 OK", &["content-encoding: Identity "], b"ok"))).await;

    let fetched = client(&server.url).get("/v1/x", &[], LIMITS).await.unwrap();

    assert_eq!(fetched.body, b"ok");
}

#[tokio::test]
async fn a_dribbling_body_runs_into_the_deadline() {
    let server = serve(|_| {
        Reply::Dribble(
            b"HTTP/1.1 200 OK\r\n\r\n".to_vec(),
            Duration::from_millis(20),
        )
    })
    .await;
    let limits = Limits {
        deadline: Duration::from_millis(300),
        // Each byte comes well within it, so only the deadline ends the fetch.
        silence: Duration::from_millis(200),
        max_bytes: 1 << 20,
    };

    // Bounded here too, so a deadline missing from the body fails the test rather than hanging it.
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        client(&server.url).get("/v1/x", &[], limits),
    )
    .await
    .expect("the deadline ends the fetch")
    .unwrap_err();

    assert!(matches!(error, FetchError::Deadline(_)), "{error:?}");
}

#[tokio::test]
async fn a_silence_ends_the_fetch_before_the_head_or_within_the_body() {
    let limits = Limits {
        silence: Duration::from_millis(200),
        ..LIMITS
    };
    for reply in [
        || Reply::Silent,
        || {
            Reply::Dribble(
                b"HTTP/1.1 200 OK\r\n\r\n".to_vec(),
                Duration::from_secs(3600),
            )
        },
    ] {
        let server = serve(move |_| reply()).await;

        let error = client(&server.url)
            .get("/v1/x", &[], limits)
            .await
            .unwrap_err();

        assert!(matches!(error, FetchError::Silent(_)), "{error:?}");
    }
}

#[tokio::test]
async fn a_request_closed_unanswered_is_a_failure_not_retried() {
    let server = serve(|_| Reply::Close).await;

    let error = client(&server.url)
        .get("/v1/x", &[], LIMITS)
        .await
        .unwrap_err();

    assert!(matches!(error, FetchError::Transport(_)), "{error:?}");
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_connection_is_reused_only_within_the_idle_timeout() {
    // Each connection answers with its number.
    let server = serve(|connection| {
        Reply::KeepAlive(
            format!("HTTP/1.1 200 OK\r\ncontent-length: 1\r\n\r\n{connection}").into_bytes(),
        )
    })
    .await;
    let settings = RailCenterSettings {
        idle_timeout: Duration::from_millis(200),
        ..settings(&server.url, Some(TOKEN))
    };
    let client = RailCenterClient::new(&settings).unwrap();

    let mut answered_on = Vec::new();
    for pause in [Duration::ZERO, Duration::ZERO, Duration::from_millis(600)] {
        tokio::time::sleep(pause).await;
        let fetched = client.get("/v1/x", &[], LIMITS).await.unwrap();
        answered_on.push(String::from_utf8(fetched.body).unwrap());
    }

    // The first two share a connection; the third, past the timeout, opens its own.
    assert_eq!(answered_on, ["0", "0", "1"]);
}

#[tokio::test]
async fn no_failure_carries_the_credential_or_the_url() {
    let silent = serve(|_| {
        Reply::Dribble(
            b"HTTP/1.1 200 OK\r\n\r\n".to_vec(),
            Duration::from_secs(3600),
        )
    })
    .await;
    let closed = serve(|_| Reply::Close).await;
    let refused = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let refused_url = format!("http://{}", refused.local_addr().unwrap());
    drop(refused);
    let limits = Limits {
        deadline: Duration::from_millis(200),
        ..LIMITS
    };

    for url in [&silent.url, &closed.url, &refused_url] {
        let error = client(url)
            .get("/v1/secret-route", &[("q", "v")], limits)
            .await
            .unwrap_err();

        for shown in [error.to_string(), format!("{error:?}")] {
            assert!(!shown.contains("AAAAAAAA"), "{shown}");
            assert!(!shown.contains("secret-route"), "{shown}");
        }
    }
}

#[tokio::test]
async fn an_https_url_is_fetched_over_tls() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("https://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
    });

    let error = client(&url).get("/v1/x", &[], LIMITS).await.unwrap_err();

    // A plaintext answer fails the handshake, so TLS was attempted.
    assert!(matches!(error, FetchError::Transport(_)), "{error:?}");
}

// Run in a child process, which the test below starts with the proxy variables set.
#[tokio::test]
#[ignore = "run by an_ambient_proxy_is_not_used"]
async fn ambient_proxy_child() {
    let server = serve(|_| Reply::Raw(response("200 OK", &[], b"direct"))).await;

    let fetched = client(&server.url).get("/v1/x", &[], LIMITS).await.unwrap();

    assert_eq!(fetched.body, b"direct");
}

#[test]
fn an_ambient_proxy_is_not_used() {
    // Nothing listens there, so a request sent through the proxy fails.
    let proxy = "http://127.0.0.1:9";
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "client::ambient_proxy_child",
            "--ignored",
            "--quiet",
        ])
        .envs(
            [
                "HTTP_PROXY",
                "http_proxy",
                "HTTPS_PROXY",
                "https_proxy",
                "ALL_PROXY",
                "all_proxy",
            ]
            .map(|name| (name, proxy)),
        )
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    // A name matching no test would pass with nothing run.
    assert!(stdout.contains("1 passed"), "{stdout}");
    assert!(output.status.success(), "{stdout}");
}
