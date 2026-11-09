use std::collections::HashMap;

use apigee_callout_proto::apigee::{MessageContext, Request, Strings, flow_variable};
use prost::Message;

use crate::common::{RailCenter, Server, client, free_port, probe, rail_apigee, wait_until};

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

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn with_rail_center_disabled_it_is_ready_and_allows_every_call() {
    let port = free_port();
    let server = tokio::task::block_in_place(|| Server::serve(rail_apigee(port), port));
    let ready = tokio::task::block_in_place(|| probe(port, &["--ready"]));
    assert_eq!(stdout(&ready), "serving\n");

    let answer = client(port)
        .await
        .process_message(call(r#"{"method": "resources/read"}"#))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(flow(&answer, "rail.decision").as_deref(), Some("allow"));
    let request = answer.request.unwrap();
    assert_eq!(request.content, r#"{"method": "resources/read"}"#);
    assert_eq!(
        request.headers,
        HashMap::from([("x-rail".to_owned(), Strings::default())])
    );
    server.terminate();
    let (code, logs) = tokio::task::block_in_place(|| server.wait_for_exit());
    assert_eq!(code, Some(0), "{logs}");
}

// The answer echoes the content, so it may be a little past the limit a request is held to.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_at_the_limit_is_answered_and_one_past_it_refused() {
    let port = free_port();
    let mut command = rail_apigee(port);
    command.env("RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB", "1");
    let _server = tokio::task::block_in_place(|| Server::serve(command, port));
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
async fn with_a_bundle_held_a_call_it_denies_is_refused_and_reported() {
    let rail_center = RailCenter::start(DENYING).await;
    let port = free_port();
    let mut command = rail_apigee(port);
    command.envs([
        ("RAIL_DISABLED", "false"),
        (
            "RAIL_CENTER_URL",
            &format!("http://{}", rail_center.address),
        ),
        ("RAIL_ALLOW_HTTP", "true"),
        ("RAIL_GATEWAY_SLUG", "gw-1"),
    ]);
    let _server = tokio::task::block_in_place(|| {
        let server = Server::serve(command, port);
        wait_until(|| probe(port, &["--ready"]).status.success());
        server
    });

    let answer = client(port)
        .await
        .process_message(call(r#"{"method": "resources/read"}"#))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(flow(&answer, "rail.decision").as_deref(), Some("refuse"));
    assert_eq!(flow(&answer, "rail.status").as_deref(), Some("403"));
    assert_eq!(
        flow(&answer, "rail.body").as_deref(),
        Some(r#"{"error": "denied by policy"}"#)
    );
    tokio::task::block_in_place(|| {
        wait_until(|| {
            let seen = rail_center.seen.lock().unwrap();
            seen.iter()
                .any(|line| line.starts_with("POST /v1/denials "))
        })
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_bundle_it_is_not_ready_and_a_call_is_forwarded() {
    let port = free_port();
    let mut command = rail_apigee(port);
    command.envs([
        ("RAIL_DISABLED", "false"),
        ("RAIL_CENTER_URL", "http://127.0.0.1:1"),
        ("RAIL_ALLOW_HTTP", "true"),
        ("RAIL_GATEWAY_SLUG", "gw-1"),
    ]);
    let _server = tokio::task::block_in_place(|| Server::serve(command, port));
    let ready = tokio::task::block_in_place(|| probe(port, &["--ready"]));
    assert_eq!(ready.status.code(), Some(1));
    assert_eq!(stdout(&ready), "not serving: NOT_SERVING\n");

    let answer = client(port)
        .await
        .process_message(call(r#"{"method": "resources/read"}"#))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(flow(&answer, "rail.decision").as_deref(), Some("allow"));
}

#[test]
fn an_unknown_argument_is_a_usage_error_and_serves_nothing() {
    let output = rail_apigee(free_port()).arg("serve").output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("usage: rail-apigee [probe [--ready]]")
    );
}
