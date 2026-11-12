//! The gateway role, served by the binary against a stub Rail Center, called as Envoy calls it.

use std::collections::HashMap;
use std::path::Path;

use envoy_types::ext_authz::v3::pb::{CheckRequest, HttpResponse};
use envoy_types::pb::envoy::config::core::v3::{HeaderMap, HeaderValue};
use envoy_types::pb::envoy::extensions::filters::http::ext_proc::v3::processing_mode::BodySendMode;
use envoy_types::pb::envoy::service::auth::v3::AttributeContext;
use envoy_types::pb::envoy::service::auth::v3::attribute_context::{HttpRequest, Request};
use envoy_types::pb::envoy::service::auth::v3::authorization_client::AuthorizationClient;
use envoy_types::pb::envoy::service::ext_proc::v3::body_mutation::Mutation;
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_client::ExternalProcessorClient;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request as Phase;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_response::Response;
use envoy_types::pb::envoy::service::ext_proc::v3::{
    HttpBody, HttpHeaders, HttpTrailers, ProcessingRequest, ProtocolConfiguration,
};
use rail_test_support::{RailCenter, Server, isolate, stdout};
use tonic::transport::{Channel, Endpoint};

use crate::common::TempSocket;

// At `enforce`, denying the tool `forbidden` and the host `denied.example`; every other call is
// allowed.
const DENYING_FORBIDDEN: &str = r#"{
    "schema_version": "2.1",
    "content_hash": "v1",
    "generated_at": "2026-07-28T20:41:07.512247Z",
    "enforcement": {"mode": "enforce"},
    "policies": [{
        "id": "5c8f1e42-0000-4000-8000-0000000000a1",
        "name": "F",
        "priority": 1,
        "condition": {"path": "request.endpoint_key", "op": "eq", "value": "/mcp#tools/call#forbidden"},
        "action": "block",
        "enabled": true
    }, {
        "id": "5c8f1e42-0000-4000-8000-0000000000a2",
        "name": "H",
        "priority": 2,
        "condition": {"path": "request.headers.host", "op": "eq", "value": "denied.example"},
        "action": "block",
        "enabled": true
    }],
    "bindings": [],
    "binding_fallback": "pass",
    "ticket_fallback": "pass",
    "ticket_issuer": null,
    "ticket_audience": null
}"#;

fn tool_call(name: &str, pad: usize) -> Vec<u8> {
    let pad = "x".repeat(pad);
    format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}","arguments":{{"pad":"{pad}"}}}}}}"#
    )
    .into_bytes()
}

// The binary in the gateway role on `socket`, `settings` beside the listener's, once ready.
fn gateway(socket: &Path, settings: &[(&str, &str)]) -> Server {
    gateway_once(socket, settings, &["probe", "--ready"])
}

// As `gateway`, once `probe` (its arguments) passes.
fn gateway_once(socket: &Path, settings: &[(&str, &str)], probe: &[&str]) -> Server {
    let command = || {
        let mut command = isolate(env!("CARGO_BIN_EXE_rail-envoy"));
        command
            .env("RAIL_LOG_FORMAT", "text")
            .env("RAIL_EXT_SOCKET", socket)
            .envs(settings.iter().copied());
        command
    };
    let mut serving = command();
    serving.arg("gateway");
    let passes = || command().args(probe).output().unwrap().status.success();
    Server::serve(serving, passes)
}

// Judging against `bundle`, from a stub Rail Center the caller keeps up.
async fn judging(
    socket: &Path,
    bundle: &'static str,
    extra: &[(&str, &str)],
) -> (Server, RailCenter) {
    let rail_center = RailCenter::start(bundle).await;
    let url = rail_center.url();
    let mut settings = vec![
        ("RAIL_CENTER_URL", url.as_str()),
        ("RAIL_CENTER_ALLOW_HTTP", "true"),
        ("RAIL_GATEWAY_SLUG", "gw-1"),
    ];
    settings.extend_from_slice(extra);
    let server = tokio::task::block_in_place(|| gateway(socket, &settings));
    (server, rail_center)
}

async fn channel(socket: &Path) -> Channel {
    Endpoint::from_shared(format!("unix:{}", socket.display()))
        .unwrap()
        .connect()
        .await
        .unwrap()
}

fn header(key: &str, value: &str) -> HeaderValue {
    HeaderValue {
        key: key.to_owned(),
        raw_value: value.as_bytes().to_vec(),
        ..Default::default()
    }
}

// A POST to `/mcp`, from an agent sending its own `x-rail` and an `_` header.
fn post(end_of_stream: bool) -> Phase {
    post_to("mcp.example", end_of_stream)
}

fn post_to(authority: &str, end_of_stream: bool) -> Phase {
    Phase::RequestHeaders(HttpHeaders {
        headers: Some(HeaderMap {
            headers: vec![
                header(":method", "POST"),
                header(":path", "/mcp?session=1"),
                header(":authority", authority),
                header("content-type", "application/json"),
                header("x-rail", "forged"),
                header("x_team", "a"),
            ],
        }),
        end_of_stream,
        ..Default::default()
    })
}

fn chunk(body: &[u8], end_of_stream: bool) -> Phase {
    Phase::RequestBody(HttpBody {
        body: body.to_vec(),
        end_of_stream,
        ..Default::default()
    })
}

// Each reply to `phases`, sent on one stream as Envoy sends them in `mode`, or the stream's error.
async fn process(
    socket: &Path,
    mode: Option<BodySendMode>,
    phases: Vec<Phase>,
) -> Result<Vec<Response>, tonic::Status> {
    let requests: Vec<ProcessingRequest> = phases
        .into_iter()
        .enumerate()
        .map(|(index, phase)| ProcessingRequest {
            request: Some(phase),
            protocol_config: mode
                .filter(|_| index == 0)
                .map(|mode| ProtocolConfiguration {
                    request_body_mode: mode.into(),
                    ..Default::default()
                }),
            ..Default::default()
        })
        .collect();
    let mut replies = ExternalProcessorClient::new(channel(socket).await)
        .process(tokio_stream::iter(requests))
        .await?
        .into_inner();
    let mut responses = Vec::new();
    while let Some(reply) = replies.message().await? {
        responses.push(reply.response.unwrap());
    }
    Ok(responses)
}

// The headers a headers response removes, or a panic naming what came instead.
fn removed(response: &Response) -> Vec<String> {
    let Response::RequestHeaders(headers) = response else {
        panic!("not a headers response: {response:?}");
    };
    let mutation = headers.response.as_ref().unwrap().header_mutation.as_ref();
    mutation.unwrap().remove_headers.clone()
}

// The status and body of an immediate response, or a panic naming what came instead.
fn refused(response: &Response) -> (i32, String) {
    let Response::ImmediateResponse(immediate) = response else {
        panic!("not refused: {response:?}");
    };
    let set = &immediate.headers.as_ref().unwrap().set_headers;
    let content_type = set.iter().filter_map(|option| option.header.as_ref());
    assert!(
        content_type
            .into_iter()
            .any(|header| header.key == "content-type" && header.raw_value == b"application/json"),
        "{immediate:?}"
    );
    let body = String::from_utf8(immediate.body.clone()).unwrap();
    (immediate.status.unwrap().code, body)
}

// The body full duplex sends back, and whether its last chunk ends the stream.
fn streamed_back(responses: &[Response]) -> (Vec<u8>, Vec<usize>, bool) {
    let mut body = Vec::new();
    let mut sizes = Vec::new();
    let mut ended = false;
    for response in responses {
        let Response::RequestBody(reply) = response else {
            panic!("not a body response: {response:?}");
        };
        let mutation = reply.response.as_ref().unwrap().body_mutation.as_ref();
        let Some(Mutation::StreamedResponse(chunk)) = mutation.unwrap().mutation.as_ref() else {
            panic!("not streamed back: {reply:?}");
        };
        body.extend_from_slice(&chunk.body);
        sizes.push(chunk.body.len());
        ended = chunk.end_of_stream;
    }
    (body, sizes, ended)
}

const WITHHELD: [&str; 2] = ["x-rail", "x_team"];

#[tokio::test(flavor = "multi_thread")]
async fn full_duplex_holds_every_answer_until_judged_then_sends_the_body_back_whole() {
    let socket = TempSocket::new("gw-duplex");
    let (_server, _rail_center) = judging(&socket.path, DENYING_FORBIDDEN, &[]).await;
    let duplex = Some(BodySendMode::FullDuplexStreamed);
    // Past one 64 KiB chunk, sent in two.
    let body = tool_call("allowed", 100 * 1024);
    let (first, second) = body.split_at(1000);

    let forwarded = process(
        &socket.path,
        duplex,
        vec![post(false), chunk(first, false), chunk(second, true)],
    )
    .await
    .unwrap();
    assert_eq!(removed(&forwarded[0]), WITHHELD);
    let (back, sizes, ended) = streamed_back(&forwarded[1..]);
    assert!(back == body);
    assert!(sizes.iter().all(|&size| size <= 64 * 1024), "{sizes:?}");
    assert!(ended);

    // With trailers, they end the body, and are answered last.
    let trailed = process(
        &socket.path,
        duplex,
        vec![
            post(false),
            chunk(&body, false),
            Phase::RequestTrailers(HttpTrailers::default()),
        ],
    )
    .await
    .unwrap();
    assert_eq!(removed(&trailed[0]), WITHHELD);
    let (back, _, ended) = streamed_back(&trailed[1..trailed.len() - 1]);
    assert!(back == body);
    assert!(!ended);
    assert!(matches!(trailed.last(), Some(Response::RequestTrailers(_))));

    // No body at all, only trailers: nothing is sent back for a body that never came.
    let bodiless = process(
        &socket.path,
        duplex,
        vec![post(false), Phase::RequestTrailers(HttpTrailers::default())],
    )
    .await
    .unwrap();
    assert_eq!(bodiless.len(), 2, "{bodiless:?}");
    assert_eq!(removed(&bodiless[0]), WITHHELD);
    assert!(matches!(bodiless[1], Response::RequestTrailers(_)));

    // `:authority` reads as `host`, as an HTTP/1 request's would.
    let by_host = process(
        &socket.path,
        duplex,
        vec![
            post_to("denied.example", false),
            chunk(&tool_call("allowed", 0), true),
        ],
    )
    .await
    .unwrap();
    assert_eq!(refused(&by_host[0]).0, 403);

    let denied = process(
        &socket.path,
        duplex,
        vec![post(false), chunk(&tool_call("forbidden", 0), true)],
    )
    .await
    .unwrap();
    assert_eq!(denied.len(), 1, "{denied:?}");
    assert_eq!(
        refused(&denied[0]),
        (403, r#"{"error": "denied by policy"}"#.to_owned())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn buffered_answers_the_headers_at_once_and_judges_the_one_body_message() {
    let socket = TempSocket::new("gw-buffered");
    let (_server, _rail_center) = judging(&socket.path, DENYING_FORBIDDEN, &[]).await;
    let buffered = Some(BodySendMode::Buffered);

    let forwarded = process(
        &socket.path,
        buffered,
        vec![post(false), chunk(&tool_call("allowed", 0), true)],
    )
    .await
    .unwrap();
    assert_eq!(forwarded.len(), 2, "{forwarded:?}");
    assert_eq!(removed(&forwarded[0]), WITHHELD);
    let Response::RequestBody(body) = &forwarded[1] else {
        panic!("not a body response: {:?}", forwarded[1]);
    };
    assert!(
        body.response
            .as_ref()
            .is_none_or(|common| common.body_mutation.is_none())
    );

    // Buffered with trailers, the body message doesn't end the stream, and is still whole.
    let denied = process(
        &socket.path,
        buffered,
        vec![
            post(false),
            chunk(&tool_call("forbidden", 0), false),
            Phase::RequestTrailers(HttpTrailers::default()),
        ],
    )
    .await
    .unwrap();
    assert_eq!(removed(&denied[0]), WITHHELD);
    assert_eq!(refused(&denied[1]).0, 403);
    let allowed = process(
        &socket.path,
        buffered,
        vec![
            post(false),
            chunk(&tool_call("allowed", 0), false),
            Phase::RequestTrailers(HttpTrailers::default()),
        ],
    )
    .await
    .unwrap();
    assert_eq!(allowed.len(), 3, "{allowed:?}");
    assert!(matches!(allowed[1], Response::RequestBody(_)));
    assert!(matches!(allowed[2], Response::RequestTrailers(_)));

    // Trailers and no body: judged when they come, and only they are answered.
    let bodiless = process(
        &socket.path,
        buffered,
        vec![
            post_to("denied.example", false),
            Phase::RequestTrailers(HttpTrailers::default()),
        ],
    )
    .await
    .unwrap();
    assert_eq!(bodiless.len(), 2, "{bodiless:?}");
    assert_eq!(removed(&bodiless[0]), WITHHELD);
    assert_eq!(refused(&bodiless[1]).0, 403);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_past_grpcs_own_4_mib_default_is_judged_whole_in_one_message() {
    let socket = TempSocket::new("gw-message");
    let (_server, _rail_center) = judging(&socket.path, DENYING_FORBIDDEN, &[]).await;
    let body = tool_call("allowed", 5 * 1024 * 1024);

    let buffered = process(
        &socket.path,
        Some(BodySendMode::Buffered),
        vec![post(false), chunk(&body, true)],
    )
    .await
    .unwrap();
    assert!(
        matches!(buffered[1], Response::RequestBody(_)),
        "{buffered:?}"
    );
    let size = i64::try_from(body.len()).unwrap();
    assert!(check(&socket.path, &[], &body, size).await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_without_a_body_is_judged_from_its_headers_and_other_modes_refused() {
    let socket = TempSocket::new("gw-modes");
    let (_server, _rail_center) = judging(&socket.path, DENYING_FORBIDDEN, &[]).await;

    let headers_only = process(&socket.path, None, vec![post(true)]).await.unwrap();
    assert_eq!(headers_only.len(), 1);
    assert_eq!(removed(&headers_only[0]), WITHHELD);

    let cannot_apply = (
        503,
        r#"{"error": "policy ruleset cannot be applied"}"#.to_owned(),
    );
    let body_first = process(
        &socket.path,
        Some(BodySendMode::FullDuplexStreamed),
        vec![chunk(&tool_call("allowed", 0), true), post(false)],
    )
    .await
    .unwrap();
    assert_eq!(refused(&body_first[0]), cannot_apply);
    for mode in [
        Some(BodySendMode::Streamed),
        Some(BodySendMode::BufferedPartial),
        Some(BodySendMode::None),
        None,
    ] {
        let replies = process(
            &socket.path,
            mode,
            vec![post(false), chunk(&tool_call("allowed", 0), true)],
        )
        .await
        .unwrap();
        assert_eq!(replies.len(), 1, "{mode:?}: {replies:?}");
        assert_eq!(refused(&replies[0]), cannot_apply, "{mode:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_streamed_body_past_the_cap_is_refused_as_soon_as_it_is() {
    let socket = TempSocket::new("gw-cap");
    let (_server, _rail_center) = judging(
        &socket.path,
        DENYING_FORBIDDEN,
        &[("RAIL_MAX_BODY_MB", "1")],
    )
    .await;
    let half = vec![b' '; 512 * 1024];

    let duplex = Some(BodySendMode::FullDuplexStreamed);

    // Refused at the third chunk, before the body ends, which it never does here.
    let replies = process(
        &socket.path,
        duplex,
        vec![
            post(false),
            chunk(&half, false),
            chunk(&half, false),
            chunk(b" ", false),
        ],
    )
    .await
    .unwrap();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(
        refused(&replies[0]),
        (413, r#"{"error": "request body too large"}"#.to_owned())
    );

    // At the cap, to the byte, it's judged.
    let short = tool_call("allowed", 0).len();
    let at_cap = tool_call("allowed", 1024 * 1024 - short);
    assert_eq!(at_cap.len(), 1024 * 1024);
    let forwarded = process(
        &socket.path,
        duplex,
        vec![post(false), chunk(&at_cap, true)],
    )
    .await
    .unwrap();
    assert!(streamed_back(&forwarded[1..]).0 == at_cap);

    // ext_authz, whole and past it.
    let past = tool_call("allowed", 1024 * 1024);
    let size = i64::try_from(past.len()).unwrap();
    assert_eq!(
        check(&socket.path, &[], &past, size).await.unwrap_err().0,
        413
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn it_is_ready_once_it_holds_a_bundle() {
    let socket = TempSocket::new("gw-ready");
    let rail_center = RailCenter::start_holding_bundle(DENYING_FORBIDDEN).await;
    let url = rail_center.url();
    let settings = [
        ("RAIL_CENTER_URL", url.as_str()),
        ("RAIL_CENTER_ALLOW_HTTP", "true"),
        ("RAIL_GATEWAY_SLUG", "gw-1"),
    ];
    let _server = tokio::task::block_in_place(|| gateway_once(&socket.path, &settings, &["probe"]));
    let ready = || {
        let mut command = isolate(env!("CARGO_BIN_EXE_rail-envoy"));
        command
            .env("RAIL_EXT_SOCKET", &socket.path)
            .args(["probe", "--ready"]);
        command.output().unwrap()
    };

    let before = tokio::task::block_in_place(ready);
    assert!(!before.status.success());
    assert_eq!(
        stdout(&before),
        "not serving: NOT_SERVING (gateway: none)\n"
    );
    rail_center.release_bundle();
    tokio::task::block_in_place(|| rail_test_support::wait_until(|| ready().status.success()));
    assert_eq!(stdout(&ready()), "serving (gateway: held)\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_in_observability_mode_is_refused() {
    let socket = TempSocket::new("gw-observe");
    let (_server, _rail_center) = judging(&socket.path, DENYING_FORBIDDEN, &[]).await;
    let request = ProcessingRequest {
        request: Some(post(true)),
        observability_mode: true,
        ..Default::default()
    };

    let mut replies = ExternalProcessorClient::new(channel(&socket.path).await)
        .process(tokio_stream::iter([request]))
        .await
        .unwrap()
        .into_inner();

    let error = replies.message().await.unwrap_err();
    assert_eq!(error.code(), tonic::Code::FailedPrecondition, "{error:?}");
}

// What ext_authz answers: the headers removed, or the refusal's status and body.
async fn check(
    socket: &Path,
    headers: &[(&str, &str)],
    body: &[u8],
    size: i64,
) -> Result<Vec<String>, (i32, String)> {
    let mut sent: HashMap<String, String> = [
        ("content-type", "application/json"),
        ("x-rail", "forged"),
        ("x_team", "a"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect();
    sent.extend(
        headers
            .iter()
            .map(|&(name, value)| (name.to_owned(), value.to_owned())),
    );
    let request = CheckRequest {
        attributes: Some(AttributeContext {
            request: Some(Request {
                http: Some(HttpRequest {
                    method: "POST".to_owned(),
                    path: "/mcp?session=1".to_owned(),
                    host: "mcp.example".to_owned(),
                    headers: sent,
                    raw_body: body.to_vec(),
                    size,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }),
    };
    let response = AuthorizationClient::new(channel(socket).await)
        .check(request)
        .await
        .unwrap()
        .into_inner();
    match response.http_response.unwrap() {
        HttpResponse::OkResponse(ok) => {
            let mut removed = ok.headers_to_remove;
            removed.sort();
            Ok(removed)
        }
        HttpResponse::DeniedResponse(denied) => {
            assert!(
                denied
                    .headers
                    .iter()
                    .filter_map(|option| option.header.as_ref())
                    .any(
                        |header| header.key == "content-type" && header.value == "application/json"
                    ),
                "{denied:?}"
            );
            Err((denied.status.unwrap().code, denied.body))
        }
        other => panic!("neither allowed nor denied: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ext_authz_judges_the_body_sent_and_never_a_body_cut_short() {
    let socket = TempSocket::new("gw-authz");
    let (_server, _rail_center) = judging(&socket.path, DENYING_FORBIDDEN, &[]).await;
    let allowed = tool_call("allowed", 0);
    let forbidden = tool_call("forbidden", 0);
    let length = |body: &[u8]| i64::try_from(body.len()).unwrap();

    assert_eq!(
        check(&socket.path, &[], &allowed, length(&allowed)).await,
        Ok(WITHHELD.map(str::to_owned).to_vec())
    );
    // An unknown size, as for a chunked request, is no reason to refuse.
    assert!(check(&socket.path, &[], &allowed, -1).await.is_ok());
    assert_eq!(
        check(&socket.path, &[], &forbidden, length(&forbidden))
            .await
            .unwrap_err()
            .0,
        403
    );

    let too_large = Err((413, r#"{"error": "request body too large"}"#.to_owned()));
    // Cut at `max_request_bytes`, flagged by Envoy, or shorter than its size.
    let partial = [("x-envoy-auth-partial-body", "true")];
    assert_eq!(check(&socket.path, &partial, &allowed, -1).await, too_large);
    assert_eq!(
        check(&socket.path, &[], &allowed, length(&allowed) + 1).await,
        too_large
    );
    // No body sent at all, its size known or not: the filter has no `with_request_body`.
    let cannot_apply = Err((
        503,
        r#"{"error": "policy ruleset cannot be applied"}"#.to_owned(),
    ));
    assert_eq!(
        check(&socket.path, &[], &[], length(&allowed)).await,
        cannot_apply
    );
    assert_eq!(check(&socket.path, &[], &[], -1).await, cannot_apply);
    // Only Envoy's header saying so makes a body partial.
    for flag in [("x-flag", "true"), ("x-envoy-auth-partial-body", "false")] {
        assert!(
            check(&socket.path, &[flag], &allowed, length(&allowed))
                .await
                .is_ok(),
            "{flag:?}"
        );
    }
    // `:authority` reads as `host`.
    let host = [(":authority", "denied.example")];
    assert_eq!(
        check(&socket.path, &host, &allowed, length(&allowed))
            .await
            .unwrap_err()
            .0,
        403
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_it_forwards_everything_removing_the_withheld_headers() {
    let socket = TempSocket::new("gw-disabled");
    let _server =
        tokio::task::block_in_place(|| gateway(&socket.path, &[("RAIL_DISABLED", "true")]));
    let forbidden = tool_call("forbidden", 0);

    let replies = process(
        &socket.path,
        Some(BodySendMode::FullDuplexStreamed),
        vec![post(false), chunk(&forbidden, true)],
    )
    .await
    .unwrap();
    assert_eq!(removed(&replies[0]), WITHHELD);
    let (back, _, ended) = streamed_back(&replies[1..]);
    assert!(back == forbidden);
    assert!(ended);

    let length = i64::try_from(forbidden.len()).unwrap();
    assert!(check(&socket.path, &[], &forbidden, length).await.is_ok());
}
