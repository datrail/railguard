use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use envoy_types::ext_authz::v3::pb::CheckRequest;
use envoy_types::pb::envoy::service::auth::v3::AttributeContext;
use envoy_types::pb::envoy::service::auth::v3::attribute_context::{HttpRequest, Request};
use envoy_types::pb::envoy::service::auth::v3::authorization_client::AuthorizationClient;
use tonic::transport::Endpoint;

use rail_test_support::Server;

use crate::common::{TempSocket, rail_envoy, serve, with_settings};

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

// The level is read with the other settings, so it applies only once they're read: `starting`
// comes before.
#[test]
fn at_warn_the_binary_logs_starting_and_its_warnings_and_no_other_info_line() {
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
    let info: Vec<_> = logs
        .lines()
        .filter(|line| line.contains(" INFO "))
        .collect();
    assert_eq!(info.len(), 1, "{logs}");
    assert!(
        info[0].contains(r#"starting version="dev" revision="unknown" role="proxy""#),
        "{logs}"
    );
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
    command.envs([
        ("RAIL_LOG_LEVEL", "debug"),
        ("RAIL_DISABLED", "false"),
        ("RAIL_CENTER_URL", "http://127.0.0.1:1"),
        ("RAIL_CENTER_ALLOW_HTTP", "true"),
        ("RAIL_HOST_ID", "host-1"),
        ("RAIL_SANDBOX_NAME", "sandbox-1"),
        ("RAIL_PROXY_PROTECTED_HOSTS", "mcp.example.com"),
    ]);
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
            && protected.contains(r#"set="x-rail-status" status="issuer-unreachable""#)
            && protected.contains(r#"upstream="tls" connect="mcp.example.com""#)
            && protected.contains(r#"removed="x-rail""#),
        "{protected}"
    );
    let other = decided("other.example");
    assert!(!other.contains("upstream="), "{other}");
    assert!(!logs.contains("forged"), "{logs}");
}

// `RAIL_LOG_FORMAT` is read before `starting`, and a refusal is a line of its own.
#[test]
fn under_gcp_every_line_is_json_from_starting_to_each_refusal() {
    let mut command = rail_envoy(Path::new("relative.sock"));
    command
        .env("RAIL_LOG_FORMAT", "gcp")
        .env("RAIL_LOG_LEVEL", "loud");

    let output = command.output().unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let logs = String::from_utf8(output.stderr).unwrap();
    let lines: Vec<_> = logs.lines().collect();
    assert_eq!(lines.len(), 3, "{logs}");
    assert!(
        lines.iter().all(|line| line.starts_with(r#"{"time":""#)),
        "{logs}"
    );
    assert!(
        lines[0]
            .contains(r#""severity":"INFO","target":"rail_runtime::logging","message":"starting""#),
        "{logs}"
    );
    for (line, refusal) in [
        (
            lines[1],
            "RAIL_LOG_LEVEL must be error, warn, info, debug or trace, got: loud",
        ),
        (lines[2], "RAIL_EXT_SOCKET must be an absolute path"),
    ] {
        assert!(
            line.contains(&format!(
                r#""severity":"ERROR","target":"rail_runtime::logging","message":"invalid configuration","refusal":"{refusal}"}}"#
            )),
            "{logs}"
        );
    }
}

// Unset, it's JSON, or Cloud Logging's on Cloud Run, logged as detected.
#[test]
fn unset_the_format_is_json_or_on_cloud_run_gcp() {
    for (platform, key, source) in [
        (None, "level", "default"),
        (Some("K_SERVICE"), "severity", "detected"),
    ] {
        let socket = TempSocket::new("detected");
        let mut command = rail_envoy(&socket.path);
        command.env_remove("RAIL_LOG_FORMAT");
        if let Some(platform) = platform {
            command.env(platform, "proxy");
        }
        let mut server = serve(command, &socket.path);
        server.terminate();
        server.wait_for_exit(Duration::from_secs(2));

        let logs = server.read_logs();
        let lines: Vec<_> = logs.lines().collect();
        assert!(
            lines[0].contains(&format!(
                r#""{key}":"INFO","target":"rail_runtime::logging","message":"starting""#
            )),
            "{logs}"
        );
        let format = lines
            .iter()
            .find(|line| line.contains(r#""name":"RAIL_LOG_FORMAT""#))
            .unwrap_or_else(|| panic!("{logs}"));
        let expected = if platform.is_some() { "gcp" } else { "json" };
        assert!(
            format.contains(&format!(r#""value":"{expected}","source":"{source}""#)),
            "{logs}"
        );
    }
}

// Before the settings, but already in the format: one error line, and nothing else.
#[test]
fn under_json_a_usage_error_is_one_error_line() {
    let mut command = with_settings(Path::new("/run/rail/unused.sock"));
    command.env("RAIL_LOG_FORMAT", "json").arg("gateway");

    let output = command.output().unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let logs = String::from_utf8(output.stderr).unwrap();
    let lines: Vec<_> = logs.lines().collect();
    assert_eq!(lines.len(), 1, "{logs}");
    assert!(lines[0].starts_with(r#"{"timestamp":""#), "{logs}");
    assert!(
        lines[0].ends_with(
            r#""level":"ERROR","target":"rail_runtime::logging","message":"invalid arguments","reason":"unknown arguments [\"gateway\"]; usage: rail-envoy proxy | rail-envoy probe [--ready]"}"#
        ),
        "{logs}"
    );
}
