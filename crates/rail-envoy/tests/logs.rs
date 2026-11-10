use std::collections::HashMap;
use std::time::{Duration, Instant};

use envoy_types::ext_authz::v3::pb::CheckRequest;
use envoy_types::pb::envoy::service::auth::v3::AttributeContext;
use envoy_types::pb::envoy::service::auth::v3::attribute_context::{HttpRequest, Request};
use envoy_types::pb::envoy::service::auth::v3::authorization_client::AuthorizationClient;
use tonic::transport::Endpoint;

use rail_test_support::Server;

use crate::common::{TempSocket, rail_envoy, serve};

// The fields after the target, of the one line holding `needle`: the first ": " ends the target.
fn find_fields<'a>(logs: &'a str, needle: &str) -> &'a str {
    let lines: Vec<_> = logs.lines().filter(|line| line.contains(needle)).collect();
    assert_eq!(lines.len(), 1, "{logs}");
    let (_, fields) = lines[0]
        .split_once(": ")
        .unwrap_or_else(|| panic!("{logs}"));
    fields
}

// The real binary, so it fails if `init` doesn't install the formatter.
#[test]
fn the_log_escapes_a_setting_value_and_an_unread_name() {
    let temp = TempSocket::new("logs");
    let dir = temp.path.parent().unwrap();
    let socket = dir.join("e\u{1b}[31m\"x.sock");
    let mut command = rail_envoy(&socket);
    command.env("RAIL_TYPO\nfake", "1");
    let mut server = Server::spawn(command);
    // Bound only once SIGTERM is handled.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "the server never bound");
        std::thread::sleep(Duration::from_millis(20));
    }

    server.terminate();

    assert_eq!(server.wait_for_exit(Duration::from_secs(2)), Some(Some(0)));
    let logs = server.read_logs();
    assert_eq!(
        find_fields(&logs, "name=\"RAIL_EXT_SOCKET\""),
        format!(
            r#"setting name="RAIL_EXT_SOCKET" value="{}/e\u{{1b}}[31m\"x.sock" source="env""#,
            dir.display()
        )
    );
    assert_eq!(
        find_fields(&logs, "set but not read"),
        r#"set but not read name="RAIL_TYPO\u{a}fake""#
    );
}

// The level is read with the other settings, so it applies only once they're read.
#[test]
fn at_warn_the_binary_logs_its_warnings_and_no_info_line() {
    let socket = TempSocket::new("warn");
    let mut command = rail_envoy(&socket.path);
    command.env("RAIL_LOG_LEVEL", "warn");
    let mut server = serve(command, &socket.path);

    server.terminate();

    assert_eq!(server.wait_for_exit(Duration::from_secs(2)), Some(Some(0)));
    let logs = server.read_logs();
    assert!(
        logs.lines()
            .any(|line| line.contains(" WARN ") && line.contains(r#"name="RAIL_DISABLED""#)),
        "{logs}"
    );
    assert!(!logs.contains(" INFO "), "{logs}");
}

fn check_request(host: &str, headers: &[(&str, &str)]) -> CheckRequest {
    let headers: HashMap<String, String> = headers
        .iter()
        .map(|&(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    CheckRequest {
        attributes: Some(AttributeContext {
            request: Some(Request {
                http: Some(HttpRequest {
                    host: host.to_owned(),
                    headers,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }),
    }
}

// Each request's decision is logged at DEBUG, the binary's own level setting deciding it.
#[tokio::test(flavor = "multi_thread")]
async fn at_debug_each_request_is_logged_with_what_it_gets() {
    let socket = TempSocket::new("debug");
    let mut command = rail_envoy(&socket.path);
    command
        .env("RAIL_LOG_LEVEL", "debug")
        .env("RAIL_PROXY_PROTECTED_HOSTS", "mcp.example.com");
    let mut server = tokio::task::block_in_place(|| serve(command, &socket.path));
    let channel = Endpoint::from_shared(format!("unix:{}", socket.path.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = AuthorizationClient::new(channel);
    for request in [
        check_request("mcp.example.com", &[("x-rail", "forged")]),
        check_request("other.example", &[]),
    ] {
        client.check(request).await.unwrap();
    }

    server.terminate();

    let exit = tokio::task::block_in_place(|| server.wait_for_exit(Duration::from_secs(2)));
    assert_eq!(exit, Some(Some(0)));
    let logs = server.read_logs();
    let decided = |host: &str| {
        logs.lines()
            .find(|line| {
                line.contains("request headers decided")
                    && line.contains(&format!(r#"authority="{host}""#))
            })
            .unwrap_or_else(|| panic!("{logs}"))
            .to_owned()
    };
    let protected = decided("mcp.example.com");
    assert!(
        protected.contains(" DEBUG ")
            && protected.contains(r#"callout="ext_authz""#)
            && protected.contains(r#"upstream="tls""#)
            && protected.contains(r#"removed="x-rail,x-rail-status""#),
        "{protected}"
    );
    let other = decided("other.example");
    assert!(!other.contains("upstream="), "{other}");
}
