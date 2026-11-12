use std::collections::HashMap;

use std::process::Command;
use std::time::{Duration, Instant};

use apigee_callout_proto::apigee::{MessageContext, Request, Strings, flow_variable};
use prost::Message;
use rail_apigee::{MAX_ANSWERED_BYTES, READY};
use tonic::transport::Endpoint;
use tonic_health::pb::HealthCheckRequest;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;

use rail_test_support::{Server, stdout, wait_until};

use crate::common::{RailCenter, client, free_port, probe, rail_apigee, serve, with_settings};

// At `enforce`, denying every call without a ticket.
const DENYING: &str = r#"{
    "schema_version": "2.1",
    "content_hash": "v1",
    "generated_at": "2026-07-28T20:41:07.512247Z",
    "enforcement": {"mode": "enforce"},
    "policies": [{
        "id": "5c8f1e42-0000-4000-8000-0000000000a1",
        "name": "P",
        "priority": 1,
        "condition": {"path": "ticket", "op": "not_existing"},
        "action": "block",
        "enabled": true
    }],
    "bindings": [],
    "binding_fallback": "pass",
    "ticket_fallback": "pass",
    "ticket_issuer": null,
    "ticket_audience": null
}"#;

// Denying only by a header item that isn't the first, or by the decoded path.
const BY_HEADER_OR_PATH: &str = r#"{
    "schema_version": "2.1",
    "content_hash": "v2",
    "generated_at": "2026-07-28T20:41:07.512247Z",
    "enforcement": {"mode": "enforce"},
    "policies": [{
        "id": "5c8f1e42-0000-4000-8000-0000000000a1",
        "name": "H",
        "priority": 1,
        "condition": {"path": "request.headers.x-team", "op": "contains", "value": "b"},
        "action": "block",
        "enabled": true
    }, {
        "id": "5c8f1e42-0000-4000-8000-0000000000a2",
        "name": "K",
        "priority": 2,
        "condition": {"path": "request.endpoint_key", "op": "eq", "value": "/mcp#tools/call#t"},
        "action": "block",
        "enabled": true
    }],
    "bindings": [],
    "binding_fallback": "pass",
    "ticket_fallback": "pass",
    "ticket_issuer": null,
    "ticket_audience": null
}"#;

const CONTENT: &str = r#"{"method": "resources/read"}"#;

fn call(content: &str) -> MessageContext {
    MessageContext {
        request: Some(Request {
            uri: "/mcp".to_owned(),
            verb: "POST".to_owned(),
            content: content.to_owned(),
            headers: HashMap::from([(
                "x-rail".to_owned(),
                Strings {
                    strings: vec!["not-a-ticket".to_owned()],
                },
            )]),
            ..Request::default()
        }),
        ..MessageContext::default()
    }
}

fn flow(answer: &MessageContext, name: &str) -> Option<String> {
    match &answer.additional_flow_variables.get(name)?.value {
        Some(flow_variable::Value::String(text)) => Some(text.clone()),
        _ => None,
    }
}

// The content as received, and only `x-rail` removed.
fn assert_echoed(answer: &MessageContext) {
    let request = answer.request.as_ref().unwrap();
    assert_eq!(request.content, CONTENT);
    assert_eq!(
        request.headers,
        HashMap::from([("x-rail".to_owned(), Strings::default())])
    );
}

fn with_rail_center(port: u16, url: &str) -> Command {
    let mut command = rail_apigee(port);
    command.envs([
        ("RAIL_DISABLED", "false"),
        ("RAIL_CENTER_URL", url),
        ("RAIL_CENTER_ALLOW_HTTP", "true"),
        ("RAIL_GATEWAY_SLUG", "gw-1"),
    ]);
    command
}

#[tokio::test(flavor = "multi_thread")]
async fn with_rail_center_disabled_it_is_ready_and_allows_every_call() {
    let port = free_port();
    let server = tokio::task::block_in_place(|| serve(rail_apigee(port), port));
    let ready = tokio::task::block_in_place(|| probe(port, &["--ready"]));
    assert_eq!(stdout(&ready), "serving (gateway: disabled)\n");

    let answer = client(port)
        .await
        .process_message(call(CONTENT))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(flow(&answer, "rail.decision").as_deref(), Some("allow"));
    assert_echoed(&answer);
    server.terminate();
    let (code, logs) = tokio::task::block_in_place(|| read_exit(server));
    assert_eq!(code, Some(0), "{logs}");
}

// The answer echoes the content, so it may be a little past the limit a request is held to.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_at_the_limit_is_answered_and_one_past_it_refused() {
    let port = free_port();
    let mut command = rail_apigee(port);
    command.env("RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB", "1");
    let _server = tokio::task::block_in_place(|| serve(command, port));
    let limit = 1 << 20;
    let mut at_limit = call(&"a".repeat(limit));
    // The framing's length prefixes are the same size either side of the limit.
    let framing = at_limit.encoded_len() - limit;
    at_limit.request.as_mut().unwrap().content = "a".repeat(limit - framing);
    assert_eq!(at_limit.encoded_len(), limit);
    let mut past_limit = at_limit.clone();
    past_limit.request.as_mut().unwrap().content.push('a');
    let mut client = client(port).await;

    let answer = client.process_message(at_limit).await.unwrap().into_inner();
    let refused = client.process_message(past_limit).await.unwrap_err();

    assert!(answer.encoded_len() > limit);
    assert_eq!(flow(&answer, "rail.decision").as_deref(), Some("allow"));
    assert_eq!(refused.code(), tonic::Code::OutOfRange, "{refused:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_whose_answer_apigee_could_not_take_is_refused_unjudged() {
    let port = free_port();
    let server = tokio::task::block_in_place(|| serve(rail_apigee(port), port));
    // Apigee's 4 MiB, less the answer's headroom.
    let limit = (4 << 20) - 1024;
    let mut at_limit = call(&"a".repeat(limit));
    let framing = at_limit.encoded_len() - limit;
    at_limit.request.as_mut().unwrap().content = "a".repeat(limit - framing);
    assert_eq!(at_limit.encoded_len(), limit);
    let mut past_limit = at_limit.clone();
    past_limit.request.as_mut().unwrap().content.push('a');
    let mut client = client(port).await;

    let answered = client.process_message(at_limit).await.unwrap().into_inner();
    let refused = client
        .process_message(past_limit)
        .await
        .unwrap()
        .into_inner();

    assert_eq!(flow(&answered, "rail.decision").as_deref(), Some("allow"));
    assert!(answered.encoded_len() <= 4 * 1024 * 1024);
    assert_eq!(flow(&refused, "rail.decision").as_deref(), Some("refuse"));
    assert_eq!(flow(&refused, "rail.status").as_deref(), Some("503"));
    assert!(refused.request.unwrap().content.is_empty());
    server.terminate();
    let (_, logs) = tokio::task::block_in_place(|| read_exit(server));
    assert_eq!(logs.matches("refused unjudged").count(), 1, "{logs}");
}

#[tokio::test(flavor = "multi_thread")]
async fn with_a_bundle_held_a_call_it_denies_is_refused_and_reported() {
    let rail_center = RailCenter::start(DENYING).await;
    let port = free_port();
    let mut command = with_rail_center(port, &rail_center.url());
    command.env("RAIL_LOG_LEVEL", "debug");
    let server = tokio::task::block_in_place(|| {
        let server = serve(command, port);
        wait_until(|| probe(port, &["--ready"]).status.success());
        server
    });

    let answer = client(port)
        .await
        .process_message(call(CONTENT))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(flow(&answer, "rail.decision").as_deref(), Some("refuse"));
    assert_eq!(flow(&answer, "rail.status").as_deref(), Some("403"));
    assert_eq!(
        flow(&answer, "rail.body").as_deref(),
        Some(r#"{"error": "denied by policy"}"#)
    );
    assert_echoed(&answer);
    tokio::task::block_in_place(|| {
        wait_until(|| {
            let seen = rail_center.seen.lock().unwrap();
            seen.iter()
                .any(|line| line.starts_with("POST /v1/denials "))
        })
    });
    server.terminate();
    let (_, logs) = tokio::task::block_in_place(|| read_exit(server));
    assert!(
        logs.contains("DEBUG rail_runtime::denial: denial reported"),
        "{logs}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_bundle_it_is_not_ready_and_a_call_is_forwarded() {
    let port = free_port();
    let command = with_rail_center(port, "http://127.0.0.1:1");
    let _server = tokio::task::block_in_place(|| serve(command, port));
    let ready = tokio::task::block_in_place(|| probe(port, &["--ready"]));
    assert_eq!(ready.status.code(), Some(1));
    assert_eq!(stdout(&ready), "not serving: NOT_SERVING (gateway: none)\n");

    let answer = client(port)
        .await
        .process_message(call(CONTENT))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(flow(&answer, "rail.decision").as_deref(), Some("allow"));
    assert_echoed(&answer);
}

#[test]
fn an_unknown_argument_is_a_usage_error_and_serves_nothing() {
    let output = with_settings(free_port()).arg("serve").output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        r#"unknown arguments ["serve"]; usage: rail-apigee gateway | rail-apigee probe [--ready]"#
    ));
}

// The role has no default, and a role this binary doesn't carry is refused like any argument.
#[test]
fn with_no_role_or_another_binarys_it_serves_nothing() {
    for (arguments, expected) in [
        (
            &[][..],
            "no role given; usage: rail-apigee gateway | rail-apigee probe",
        ),
        (&["proxy"][..], r#"unknown arguments ["proxy"]; usage:"#),
    ] {
        let output = with_settings(free_port()).args(arguments).output().unwrap();

        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{stderr}");
        assert!(!stderr.contains("starting"), "{stderr}");
    }
}

// It reads only the port, so a setting only the server reads, refused there, can't fail it.
#[tokio::test(flavor = "multi_thread")]
async fn the_probe_reads_only_the_port() {
    let port = free_port();
    let _server = tokio::task::block_in_place(|| serve(rail_apigee(port), port));
    let mut command = with_settings(port);
    command
        .args(["probe", "--ready"])
        .env("RAIL_DISABLED", "false")
        .env("RAIL_GATEWAY_PORT", "1")
        .env("RAIL_TYPO", "x");

    let output = tokio::task::block_in_place(|| command.output().unwrap());

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}

#[test]
fn a_port_in_use_is_refused_as_configuration() {
    let taken = std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).unwrap();
    let port = taken.local_addr().unwrap().port();

    let output = rail_apigee(port).output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("can't listen on the port"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_gateway_judges_every_header_item_and_the_decoded_path() {
    let rail_center = RailCenter::start(BY_HEADER_OR_PATH).await;
    let port = free_port();
    let _server = tokio::task::block_in_place(|| {
        let server = serve(with_rail_center(port, &rail_center.url()), port);
        wait_until(|| probe(port, &["--ready"]).status.success());
        server
    });
    let mut client = client(port).await;
    let mut by_header = call(CONTENT);
    let headers = &mut by_header.request.as_mut().unwrap().headers;
    headers.insert(
        "x-team".to_owned(),
        Strings {
            strings: vec!["a".to_owned(), "b".to_owned()],
        },
    );
    let mut by_path = call(r#"{"method": "tools/call", "params": {"name": "t"}}"#);
    by_path.request.as_mut().unwrap().uri = "/m%63p?x=1".to_owned();

    let allowed = client.process_message(call(CONTENT)).await.unwrap();
    let by_header = client.process_message(by_header).await.unwrap();
    let by_path = client.process_message(by_path).await.unwrap();

    assert_eq!(
        flow(allowed.get_ref(), "rail.decision").as_deref(),
        Some("allow")
    );
    assert_eq!(
        flow(by_header.get_ref(), "rail.decision").as_deref(),
        Some("refuse")
    );
    assert_eq!(
        flow(by_path.get_ref(), "rail.decision").as_deref(),
        Some("refuse")
    );
}

// Only readiness checks bring it up to date, so only they may see the bundle arrive.
#[tokio::test(flavor = "multi_thread")]
async fn readiness_checks_alone_see_the_bundle_arrive() {
    let rail_center = RailCenter::start_holding_bundle(DENYING).await;
    let port = free_port();
    tokio::task::block_in_place(|| {
        let _server = serve(with_rail_center(port, &rail_center.url()), port);
        assert_eq!(
            stdout(&probe(port, &["--ready"])),
            "not serving: NOT_SERVING (gateway: none)\n"
        );

        rail_center.release_bundle();

        wait_until(|| probe(port, &["--ready"]).status.success());
        assert_eq!(
            stdout(&probe(port, &["--ready"])),
            "serving (gateway: held)\n"
        );
    });
}

// The watches open hold the drain until the deadline.
#[tokio::test(flavor = "multi_thread")]
async fn on_sigterm_both_turn_not_serving_and_it_exits_at_the_deadline() {
    let port = free_port();
    let server = tokio::task::block_in_place(|| serve(rail_apigee(port), port));
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut health = HealthClient::new(channel);
    let mut watches = Vec::new();
    for service in ["", READY] {
        let request = HealthCheckRequest {
            service: service.to_owned(),
        };
        let mut watch = health.watch(request).await.unwrap().into_inner();
        let first = watch.message().await.unwrap().unwrap();
        assert_eq!(first.status(), ServingStatus::Serving, "{service:?}");
        watches.push(watch);
    }
    let started = Instant::now();

    server.terminate();

    for watch in &mut watches {
        let next = watch.message().await.unwrap().unwrap();
        assert_eq!(next.status(), ServingStatus::NotServing);
    }
    let (code, logs) = tokio::task::block_in_place(|| read_exit(server));
    assert_eq!(code, Some(0), "{logs}");
    assert!(logs.contains("shut down at the deadline"), "{logs}");
    assert!(started.elapsed() < Duration::from_secs(8));
}

// The exit code and what it logged; it must exit within 10 s.
fn read_exit(mut server: Server) -> (Option<i32>, String) {
    let code = server
        .wait_for_exit(Duration::from_secs(10))
        .expect("the server never exited");
    (code, server.read_logs())
}

#[tokio::test(flavor = "multi_thread")]
async fn with_rail_center_enabled_a_request_whose_answer_apigee_could_not_take_is_refused() {
    let port = free_port();
    let command = with_rail_center(port, "http://127.0.0.1:1");
    let _server = tokio::task::block_in_place(|| serve(command, port));

    let refused = client(port)
        .await
        .process_message(call(&"a".repeat(MAX_ANSWERED_BYTES)))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(flow(&refused, "rail.status").as_deref(), Some("503"));
}
