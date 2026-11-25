//! The proxy's capture, started as the binary starts it, and by the binary, against a stub Rail
//! Center serving a capture plan, its ext_authz and ext_proc called as Envoy calls them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use envoy_types::ext_authz::v3::pb::{Authorization, CheckRequest};
use envoy_types::pb::envoy::config::core::v3::{HeaderMap, HeaderValue};
use envoy_types::pb::envoy::extensions::filters::http::ext_proc::v3::processing_mode::BodySendMode;
use envoy_types::pb::envoy::service::auth::v3::AttributeContext;
use envoy_types::pb::envoy::service::auth::v3::attribute_context::{HttpRequest, Request};
use envoy_types::pb::envoy::service::auth::v3::authorization_client::AuthorizationClient;
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_client::ExternalProcessorClient;
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessorServer;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request as Phase;
use envoy_types::pb::envoy::service::ext_proc::v3::{
    HttpBody, HttpHeaders, ProcessingRequest, ProtocolConfiguration,
};
use rail_envoy::{Callouts, ProxyConfig};
use rail_runtime::{Proxy, start_proxy};
use rail_test_support::{RailCenter, protect};
use serde_json::Value;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Channel, Endpoint, Server};

use crate::common::{TempSocket, rail_envoy, serve};

const PLANS: &str = include_str!("../../../vectors/parse_plan_conformance_cases.json");

// The parse vectors' plan: `e1` takes a `create_or_update_file` call to GitHub's `/mcp`, its
// record keyed by the repository; its windows a day long, so a run straddles two only at midnight
// UTC.
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
        "params": {
            "name": "create_or_update_file",
            "arguments": {"repo": repo, "content": content},
        },
    })
    .to_string()
    .into_bytes()
}

// The worked example's agent's settings, against `url`, with a 2 MiB cap.
fn settings(url: &str) -> [(&str, &str); 7] {
    [
        ("RAIL_DISABLED", "false"),
        ("RAIL_CENTER_URL", url),
        ("RAIL_CENTER_ALLOW_HTTP", "true"),
        ("RAIL_HOST_ID", "host-7f3c"),
        ("RAIL_SANDBOX_NAME", "agent-finretail"),
        ("RAIL_PROXY_PROTECTED_HOSTS", "mcp.example.com"),
        ("RAIL_MAX_BODY_MB", "2"),
    ]
}

// The proxy for the worked example's agent, against `rail_center`, as its settings start it.
fn start(rail_center: &RailCenter) -> Proxy {
    let url = rail_center.url();
    let mut pairs = settings(&url).to_vec();
    pairs.push(("RAIL_EXT_SOCKET", "/run/rail/ext.sock"));
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
    let padding = 2 * MIB - mcp_call("exact", "").len();
    let exact = mcp_call("exact", &"e".repeat(padding));
    let lower = mcp_call("lower", "x");

    check(&callouts, post(&whole, Some(whole.len()))).await;
    // Sent as text, without `pack_as_bytes`.
    let as_text = HttpRequest {
        raw_body: Vec::new(),
        body: String::from_utf8(whole.clone()).unwrap(),
        ..post(&whole, Some(whole.len()))
    };
    check(&callouts, as_text).await;
    check(&callouts, post(&within, Some(within.len()))).await;
    // Exactly at the cap; and declaring less than it sent, counted as sent.
    check(&callouts, post(&exact, Some(exact.len()))).await;
    check(&callouts, post(&lower, Some(1))).await;
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

    let documents = documents(&rail_center);
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
    assert_eq!(e1("exact")["measures"]["request_bytes"], exact.len());
    assert_eq!(e1("lower")["measures"]["request_bytes"], lower.len());
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

// A POST to GitHub's `/mcp` as ext_proc sends its headers, declaring `Content-Length` as
// `declared`, if any, and ending the stream if `ended`.
fn headers(declared: Option<usize>, ended: bool) -> Phase {
    let header = |key: &str, value: &str| HeaderValue {
        key: key.to_owned(),
        raw_value: value.as_bytes().to_vec(),
        ..Default::default()
    };
    let mut sent = vec![
        header(":authority", "api.github.com"),
        header(":method", "POST"),
        header(":path", "/mcp"),
    ];
    sent.extend(declared.map(|length| header("content-length", &length.to_string())));
    Phase::RequestHeaders(HttpHeaders {
        headers: Some(HeaderMap { headers: sent }),
        end_of_stream: ended,
        ..Default::default()
    })
}

fn body(sent: &[u8], ended: bool) -> Phase {
    Phase::RequestBody(HttpBody {
        body: sent.to_vec(),
        end_of_stream: ended,
        ..Default::default()
    })
}

// One stream's phases as Envoy sends them, its first message naming `mode`, if any; returns once
// the stream has ended.
async fn process(channel: &Channel, mode: Option<BodySendMode>, phases: Vec<Phase>) {
    let config = mode.map(|mode| ProtocolConfiguration {
        request_body_mode: mode.into(),
        ..Default::default()
    });
    let messages: Vec<ProcessingRequest> = phases
        .into_iter()
        .enumerate()
        .map(|(index, phase)| ProcessingRequest {
            request: Some(phase),
            protocol_config: config.filter(|_| index == 0),
            ..Default::default()
        })
        .collect();
    let mut replies = ExternalProcessorClient::new(channel.clone())
        .process(tokio_stream::iter(messages))
        .await
        .unwrap()
        .into_inner();
    while replies.message().await.unwrap().is_some() {}
}

#[tokio::test(flavor = "multi_thread")]
async fn ext_proc_calls_are_kept_from_their_buffered_body() {
    let rail_center = RailCenter::start_with_plan(worked_example()).await;
    let Proxy {
        tickets,
        capture,
        stop,
    } = start(&rail_center);
    tokio::task::block_in_place(|| rail_test_support::wait_until(|| capture.is_capturing()));
    let callouts = Callouts {
        tickets: Some(tickets),
        protected_hosts: Arc::new(protect("mcp.example.com")),
        capture: Some(capture),
    };
    let socket = TempSocket::new("ext-proc-capture");
    let listener = UnixListener::bind(&socket.path).unwrap();
    let server = tokio::spawn(
        Server::builder()
            .add_service(ExternalProcessorServer::new(callouts))
            .serve_with_incoming(UnixListenerStream::new(listener)),
    );
    let channel = Endpoint::from_shared(format!("unix:{}", socket.path.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let whole = mcp_call("docs", "x");
    let large = mcp_call("uploads", &"r".repeat(3 * MIB));
    let partial = Some(BodySendMode::BufferedPartial);
    let buffered = Some(BodySendMode::Buffered);
    let sized = || headers(Some(whole.len()), false);

    // Ended before its body: first, so it's handed over before the later streams end.
    process(&channel, buffered, vec![sized()]).await;
    let ended = body(&whole, true);
    process(&channel, partial, vec![sized(), ended]).await;
    // Buffered, the one message is the whole body, though trailers follow it.
    let trailed = body(&whole, false);
    process(&channel, buffered, vec![headers(None, false), trailed]).await;
    // Partial, trailers following, but whole by its `Content-Length`.
    let declared_whole = body(&whole, false);
    process(&channel, partial, vec![sized(), declared_whole]).await;
    // An empty body, ending the stream at its headers: counted, its body read.
    process(&channel, partial, vec![headers(Some(0), true)]).await;
    // Partial: cut short of its `Content-Length`; chunked, within the cap and past it.
    let short = body(&whole[..10], false);
    process(&channel, partial, vec![sized(), short]).await;
    let chunked = body(&whole[..10], false);
    process(&channel, partial, vec![headers(None, false), chunked]).await;
    let past = body(&large, false);
    process(&channel, partial, vec![headers(None, false), past]).await;
    // Not sent: `NONE`, as when no mode is named, and a streamed mode, whose body isn't kept.
    process(&channel, None, vec![sized()]).await;
    let streamed = Some(BodySendMode::Streamed);
    let chunk = body(&whole, true);
    process(&channel, streamed, vec![sized(), chunk]).await;
    server.abort();
    tokio::task::spawn_blocking(move || stop.stop())
        .await
        .unwrap();

    let documents = documents(&rail_center);
    let [document] = documents.as_slice() else {
        panic!("{documents:?}");
    };
    let records = document["records"].as_array().unwrap();
    let docs = records
        .iter()
        .find(|record| record["context_keys"]["destination"] == "docs")
        .unwrap_or_else(|| panic!("{document}"));
    assert_eq!(docs["measures"]["call_count"], 3);
    assert_eq!(docs["measures"]["request_bytes"], 3 * whole.len());
    let calls: u64 = records
        .iter()
        .map(|record| record["measures"]["call_count"].as_u64().unwrap())
        .sum();
    assert_eq!(calls, 10, "{document}");
    let unread = &document["truncated"]["body_unread"];
    assert_eq!(
        (
            &unread["not_sent"],
            &unread["cut_short"],
            &unread["past_cap"]
        ),
        (&Value::from(2), &Value::from(3), &Value::from(1)),
        "{document}"
    );
}

// Each call record document Rail Center was sent.
fn documents(rail_center: &RailCenter) -> Vec<Value> {
    let posted = rail_center.posted.lock().unwrap();
    posted
        .iter()
        .filter(|(line, _)| line.starts_with("POST /v1/call-records"))
        .map(|(_, body)| serde_json::from_slice(body).unwrap())
        .collect()
}

// The binary's own wiring: its cap, past the default and kept, the sender its ext_authz hands calls
// to, and the window sent at SIGTERM.
#[tokio::test(flavor = "multi_thread")]
async fn the_binary_captures_within_its_cap_and_sends_the_window_at_sigterm() {
    let rail_center = RailCenter::start_with_plan(worked_example()).await;
    let socket = TempSocket::new("capture");
    let mut command = rail_envoy(&socket.path);
    command.envs(settings(&rail_center.url()));
    let mut server = tokio::task::block_in_place(|| serve(command, &socket.path));
    tokio::task::block_in_place(|| server.wait_for_log("holding a capture plan"));
    let within = mcp_call("uploads", &"w".repeat(3 * MIB / 2));
    let past = mcp_call("uploads", &"p".repeat(3 * MIB));
    let channel = Endpoint::from_shared(format!("unix:{}", socket.path.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = AuthorizationClient::new(channel);
    for body in [&within, &past] {
        let request = CheckRequest {
            attributes: Some(AttributeContext {
                request: Some(Request {
                    http: Some(post(body, Some(body.len()))),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        };
        client.check(request).await.unwrap();
    }

    server.terminate();

    assert_eq!(server.wait_for_exit(Duration::from_secs(5)), Some(Some(0)));
    let documents = documents(&rail_center);
    let [document] = documents.as_slice() else {
        panic!("{documents:?}");
    };
    assert_eq!(
        document["records"][0]["context_keys"]["destination"],
        "uploads"
    );
    assert_eq!(
        document["records"][0]["measures"]["request_bytes"],
        within.len()
    );
    assert_eq!(document["truncated"]["body_unread"]["past_cap"], 1);
}
