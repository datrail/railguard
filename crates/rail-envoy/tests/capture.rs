//! The proxy's capture, served by the binary against a stub Rail Center serving a capture plan,
//! called as Envoy calls it.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use envoy_types::ext_authz::v3::pb::CheckRequest;
use envoy_types::pb::envoy::service::auth::v3::AttributeContext;
use envoy_types::pb::envoy::service::auth::v3::attribute_context::{HttpRequest, Request};
use envoy_types::pb::envoy::service::auth::v3::authorization_client::AuthorizationClient;
use rail_test_support::{RailCenter, Server};
use serde_json::Value;
use tonic::transport::Endpoint;

use crate::common::{TempSocket, rail_envoy};

const PLANS: &str = include_str!("../../../vectors/parse_plan_conformance_cases.json");

// The capture design's worked example: `e1` takes a `create_or_update_file` call to GitHub's
// `/mcp`, its record keyed by the repository.
fn worked_example() -> &'static str {
    let vectors: Value = serde_json::from_str(PLANS).unwrap();
    Box::leak(vectors["plan"].to_string().into_boxed_str())
}

fn mcp_call(repo: &str) -> Vec<u8> {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "create_or_update_file", "arguments": {"repo": repo, "content": "x"}},
    })
    .to_string()
    .into_bytes()
}

// The proxy for the worked example's agent, against `rail_center`, once it's serving.
fn capturing(socket: &Path, rail_center: &RailCenter) -> Server {
    let mut command = rail_envoy(socket);
    command.envs([
        ("RAIL_DISABLED", "false"),
        ("RAIL_CENTER_URL", rail_center.url().as_str()),
        ("RAIL_CENTER_ALLOW_HTTP", "true"),
        ("RAIL_HOST_ID", "host-7f3c"),
        ("RAIL_SANDBOX_NAME", "agent-finretail"),
        ("RAIL_PROXY_PROTECTED_HOSTS", "mcp.example.com"),
    ]);
    crate::common::serve(command, socket)
}

// One ext_authz check, as Envoy sends it with `pack_as_bytes`: `sent` of a body whose
// `Content-Length` is `declared`, if any.
async fn check(socket: &Path, sent: &[u8], declared: Option<usize>) {
    let headers: HashMap<String, String> = declared
        .map(|length| ("content-length".to_owned(), length.to_string()))
        .into_iter()
        .collect();
    let request = CheckRequest {
        attributes: Some(AttributeContext {
            request: Some(Request {
                http: Some(HttpRequest {
                    method: "POST".to_owned(),
                    path: "/mcp".to_owned(),
                    host: "api.github.com".to_owned(),
                    headers,
                    raw_body: sent.to_vec(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }),
    };
    let channel = Endpoint::from_shared(format!("unix:{}", socket.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    AuthorizationClient::new(channel)
        .check(request)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn ext_authz_calls_are_counted_and_their_window_sent_at_shutdown() {
    let rail_center = RailCenter::start_with_plan(worked_example()).await;
    let socket = TempSocket::new("capture");
    let mut server = tokio::task::block_in_place(|| capturing(&socket.path, &rail_center));
    tokio::task::block_in_place(|| {
        rail_test_support::wait_until(|| {
            let seen = rail_center.seen.lock().unwrap();
            seen.iter()
                .any(|line| line.starts_with("GET /v1/capture-plan"))
        });
    });
    // The plan answered is applied on the holder's next step.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let whole = mcp_call("docs");
    let large = mcp_call(&"r".repeat(2 * 1024 * 1024));

    check(&socket.path, &whole, Some(whole.len())).await;
    // No body sent, one cut short, one past the cap, sent whole, and one cut short past it.
    check(&socket.path, b"", Some(whole.len())).await;
    check(&socket.path, &whole[..10], Some(whole.len())).await;
    check(&socket.path, &large, None).await;
    check(&socket.path, &large, Some(large.len() + 1)).await;
    server.terminate();

    assert_eq!(server.wait_for_exit(Duration::from_secs(5)), Some(Some(0)));
    let posted = rail_center.posted.lock().unwrap();
    let documents: Vec<Value> = posted
        .iter()
        .filter(|(line, _)| line.starts_with("POST /v1/call-records"))
        .map(|(_, body)| serde_json::from_slice(body).unwrap())
        .collect();
    let [document] = documents.as_slice() else {
        panic!("{documents:?}");
    };
    let e1 = &document["records"][0];
    assert_eq!(e1["entry_id"], "e1");
    assert_eq!(e1["context_keys"]["destination"], "docs");
    assert_eq!(e1["measures"]["call_count"], 1);
    assert_eq!(e1["measures"]["request_bytes"], whole.len());
    let unread = &document["truncated"]["body_unread"];
    assert_eq!(
        (
            &unread["not_sent"],
            &unread["cut_short"],
            &unread["past_cap"]
        ),
        (&Value::from(1), &Value::from(1), &Value::from(2)),
        "{document}"
    );
}
