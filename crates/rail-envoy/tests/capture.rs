//! The proxy's capture, served by the binary against a stub Rail Center serving a capture plan,
//! called as Envoy calls it.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use envoy_types::ext_authz::v3::pb::CheckRequest;
use envoy_types::pb::envoy::config::core::v3::{HeaderMap, HeaderValue};
use envoy_types::pb::envoy::service::auth::v3::AttributeContext;
use envoy_types::pb::envoy::service::auth::v3::attribute_context::{HttpRequest, Request};
use envoy_types::pb::envoy::service::auth::v3::authorization_client::AuthorizationClient;
use rail_test_support::{RailCenter, Server};
use serde_json::Value;
use tonic::transport::Endpoint;

use crate::common::{TempSocket, rail_envoy};

const PLANS: &str = include_str!("../../../vectors/parse_plan_conformance_cases.json");

// The parse vectors' plan: `e1` takes a `create_or_update_file` call to GitHub's `/mcp`, its
// record keyed by the repository; its windows a day long, so a run never straddles two.
fn worked_example() -> &'static str {
    let vectors: Value = serde_json::from_str(PLANS).unwrap();
    let mut plan = vectors["plan"].clone();
    plan["window_seconds"] = Value::from(86_400);
    Box::leak(plan.to_string().into_boxed_str())
}

fn mcp_call(repo: &str, content: &str) -> Vec<u8> {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "create_or_update_file", "arguments": {"repo": repo, "content": content}},
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
        ("RAIL_MAX_BODY_MB", "2"),
    ]);
    crate::common::serve(command, socket)
}

// A POST of `sent` to GitHub's `/mcp`, as Envoy sends it with `pack_as_bytes`, declaring
// `Content-Length` as `declared`, if any.
fn post(sent: &[u8], declared: Option<usize>) -> HttpRequest {
    let headers: HashMap<String, String> = declared
        .map(|length| ("content-length".to_owned(), length.to_string()))
        .into_iter()
        .collect();
    HttpRequest {
        method: "POST".to_owned(),
        path: "/mcp".to_owned(),
        host: "api.github.com".to_owned(),
        headers,
        raw_body: sent.to_vec(),
        size: i64::try_from(sent.len()).unwrap(),
        ..Default::default()
    }
}

async fn check(socket: &Path, http: HttpRequest) {
    let request = CheckRequest {
        attributes: Some(AttributeContext {
            request: Some(Request {
                http: Some(http),
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

const MIB: usize = 1024 * 1024;

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
    tokio::time::sleep(Duration::from_millis(500)).await;
    let whole = mcp_call("docs", "x");
    // Within the 2 MiB cap; past gRPC's own 4 MiB default.
    let within = mcp_call("uploads", &"w".repeat(3 * MIB / 2));
    let large = mcp_call("uploads", &"r".repeat(5 * MIB));
    let at_cap = vec![b' '; 2 * MIB];

    check(&socket.path, post(&whole, Some(whole.len()))).await;
    // Sent as text, without `pack_as_bytes`.
    let as_text = HttpRequest {
        raw_body: Vec::new(),
        body: String::from_utf8(whole.clone()).unwrap(),
        ..post(&whole, Some(whole.len()))
    };
    check(&socket.path, as_text).await;
    check(&socket.path, post(&within, Some(within.len()))).await;
    // Not sent: declared, or a POST of a size Envoy doesn't know.
    check(&socket.path, post(b"", Some(whole.len()))).await;
    check(
        &socket.path,
        HttpRequest {
            size: -1,
            ..post(b"", None)
        },
    )
    .await;
    // Cut short of a `Content-Length` sent as a header, or raw in `header_map`.
    check(&socket.path, post(&whole[..10], Some(whole.len()))).await;
    let raw_length = HeaderMap {
        headers: vec![HeaderValue {
            key: "content-length".to_owned(),
            raw_value: whole.len().to_string().into_bytes(),
            ..Default::default()
        }],
    };
    let in_header_map = HttpRequest {
        header_map: Some(raw_length),
        ..post(&whole[..10], None)
    };
    check(&socket.path, in_header_map).await;
    // Past the cap: sent whole, cut short, and cut at the cap of a longer body.
    check(&socket.path, post(&large, None)).await;
    check(&socket.path, post(&large, Some(large.len() + 1))).await;
    check(&socket.path, post(&at_cap, Some(at_cap.len() + 1))).await;
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
    let records = document["records"].as_array().unwrap();
    let e1 = |destination: &str| {
        records
            .iter()
            .find(|record| record["context_keys"]["destination"] == destination)
            .unwrap_or_else(|| panic!("{document}"))
    };
    assert_eq!(e1("docs")["entry_id"], "e1");
    assert_eq!(e1("docs")["measures"]["call_count"], 2);
    assert_eq!(e1("docs")["measures"]["request_bytes"], 2 * whole.len());
    assert_eq!(e1("uploads")["measures"]["request_bytes"], within.len());
    let unread = &document["truncated"]["body_unread"];
    assert_eq!(
        (
            &unread["not_sent"],
            &unread["cut_short"],
            &unread["past_cap"]
        ),
        (&Value::from(2), &Value::from(2), &Value::from(3)),
        "{document}"
    );
}
