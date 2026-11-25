//! The proxy's capture, started as the binary starts it against a stub Rail Center serving a
//! capture plan, its ext_authz called as Envoy calls it.

use std::collections::HashMap;
use std::sync::Arc;

use envoy_types::ext_authz::v3::pb::{Authorization, CheckRequest};
use envoy_types::pb::envoy::config::core::v3::{HeaderMap, HeaderValue};
use envoy_types::pb::envoy::service::auth::v3::AttributeContext;
use envoy_types::pb::envoy::service::auth::v3::attribute_context::{HttpRequest, Request};
use rail_envoy::{Callouts, ProxyConfig};
use rail_runtime::{Proxy, start_proxy};
use rail_test_support::{RailCenter, protect};
use serde_json::Value;

const PLANS: &str = include_str!("../../../vectors/parse_plan_conformance_cases.json");

// The parse vectors' plan: `e1` takes a `create_or_update_file` call to GitHub's `/mcp`, its
// record keyed by the repository; its windows a day long, so a run straddles two only at midnight UTC.
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

// The proxy for the worked example's agent, against `rail_center`, as its settings start it.
fn start(rail_center: &RailCenter) -> Proxy {
    let url = rail_center.url();
    let pairs = [
        ("RAIL_EXT_SOCKET", "/run/rail/ext.sock"),
        ("RAIL_CENTER_URL", url.as_str()),
        ("RAIL_CENTER_ALLOW_HTTP", "true"),
        ("RAIL_HOST_ID", "host-7f3c"),
        ("RAIL_SANDBOX_NAME", "agent-finretail"),
        ("RAIL_PROXY_PROTECTED_HOSTS", "mcp.example.com"),
        ("RAIL_MAX_BODY_MB", "2"),
    ];
    let (config, _) = rail_test_support::read(&pairs, ProxyConfig::read).unwrap();
    start_proxy(config.rail_center.unwrap(), config.max_body_bytes).unwrap()
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

async fn check(callouts: &Callouts, http: HttpRequest) {
    let request = CheckRequest {
        attributes: Some(AttributeContext {
            request: Some(Request {
                http: Some(http),
                ..Default::default()
            }),
            ..Default::default()
        }),
    };
    callouts.check(tonic::Request::new(request)).await.unwrap();
}

const MIB: usize = 1024 * 1024;

#[tokio::test(flavor = "multi_thread")]
async fn ext_authz_calls_are_counted_and_their_window_sent_when_stopped() {
    let rail_center = RailCenter::start_with_plan(worked_example()).await;
    let Proxy {
        tickets,
        capture,
        stop,
    } = start(&rail_center);
    // A call is handed over only once the plan is held.
    tokio::task::block_in_place(|| rail_test_support::wait_until(|| capture.is_capturing()));
    let callouts = Callouts {
        tickets: Some(tickets),
        protected_hosts: Arc::new(protect("mcp.example.com")),
        capture: Some(capture),
    };
    let whole = mcp_call("docs", "x");
    let within = mcp_call("uploads", &"w".repeat(3 * MIB / 2));
    let large = mcp_call("uploads", &"r".repeat(3 * MIB));
    let at_cap = vec![b' '; 2 * MIB];

    check(&callouts, post(&whole, Some(whole.len()))).await;
    // Sent as text, without `pack_as_bytes`.
    let as_text = HttpRequest {
        raw_body: Vec::new(),
        body: String::from_utf8(whole.clone()).unwrap(),
        ..post(&whole, Some(whole.len()))
    };
    check(&callouts, as_text).await;
    check(&callouts, post(&within, Some(within.len()))).await;
    // Not sent: declared, or a POST of a size Envoy doesn't know.
    check(&callouts, post(b"", Some(whole.len()))).await;
    check(
        &callouts,
        HttpRequest {
            size: -1,
            ..post(b"", None)
        },
    )
    .await;
    // Cut short of a `Content-Length` sent as a header, or raw in `header_map`.
    check(&callouts, post(&whole[..10], Some(whole.len()))).await;
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
    check(&callouts, in_header_map).await;
    // Past the cap: sent whole, and cut at the cap of a longer body.
    check(&callouts, post(&large, None)).await;
    check(&callouts, post(&at_cap, Some(at_cap.len() + 1))).await;
    // As the binary stops it, once its server has drained.
    tokio::task::spawn_blocking(move || stop.stop())
        .await
        .unwrap();

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
        (&Value::from(2), &Value::from(2), &Value::from(2)),
        "{document}"
    );
}
