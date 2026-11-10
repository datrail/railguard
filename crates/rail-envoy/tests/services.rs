use std::collections::HashMap;
use std::sync::Arc;

use envoy_types::ext_authz::v3::pb::{Authorization, CheckRequest, HttpResponse};
use envoy_types::pb::envoy::config::core::v3::{HeaderMap, HeaderValue, HeaderValueOption};
use envoy_types::pb::envoy::service::auth::v3::AttributeContext;
use envoy_types::pb::envoy::service::auth::v3::attribute_context::{HttpRequest, Request};
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_client::ExternalProcessorClient;
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessorServer;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request as Phase;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_response::Response;
use envoy_types::pb::envoy::service::ext_proc::v3::{
    BodyResponse, HeadersResponse, HttpBody, HttpHeaders, HttpTrailers, ProcessingRequest,
    TrailersResponse,
};
use rail_envoy::Callouts;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Endpoint, Server};

use rail_test_support::protect;

use crate::common::TempSocket;

// Each header's name and value, from `value` or `raw_value`, whichever is set.
fn read_set(options: &[HeaderValueOption]) -> Vec<(String, String)> {
    options
        .iter()
        .map(|option| {
            let header = option.header.as_ref().unwrap();
            let raw = String::from_utf8(header.raw_value.clone()).unwrap();
            (header.key.clone(), format!("{}{raw}", header.value))
        })
        .collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|&(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

async fn check(host: &str, protected: &str) -> (Vec<(String, String)>, Vec<String>) {
    let allow = Callouts {
        tickets: None,
        protected_hosts: Arc::new(protect(protected)),
    };
    let headers: HashMap<String, String> = [("x-rail-upstream", "plain"), ("accept", "*/*")]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    let request = CheckRequest {
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
    };
    let response = allow
        .check(tonic::Request::new(request))
        .await
        .unwrap()
        .into_inner();
    let Some(HttpResponse::OkResponse(ok)) = response.http_response else {
        panic!("not allowed: {response:?}");
    };
    // ext_authz reads `value`.
    assert!(ok.headers.iter().all(|option| {
        option
            .header
            .as_ref()
            .is_some_and(|header| header.raw_value.is_empty())
    }));
    (read_set(&ok.headers), ok.headers_to_remove)
}

#[tokio::test]
async fn ext_authz_routes_a_protected_host_from_its_host() {
    let (set, remove) = check("MCP.example.com:9443", "mcp.example.com").await;

    assert_eq!(
        set,
        pairs(&[
            ("x-rail-upstream", "tls"),
            (":authority", "mcp.example.com:9443")
        ])
    );
    assert_eq!(remove, ["x-rail", "x-rail-status"]);
}

#[tokio::test]
async fn ext_authz_leaves_an_unprotected_host_unrouted() {
    let (set, remove) = check("open.example.com", "mcp.example.com").await;

    assert!(set.is_empty());
    assert_eq!(remove, ["x-rail", "x-rail-status", "x-rail-upstream"]);
}

// Each phase's reply from a served ext_proc, the phases sent on one stream as Envoy sends them.
async fn send(phases: Vec<Phase>, protected: &str) -> Vec<Response> {
    let socket = TempSocket::new("ext-proc");
    let listener = UnixListener::bind(&socket.path).unwrap();
    let callouts = Callouts {
        tickets: None,
        protected_hosts: Arc::new(protect(protected)),
    };
    tokio::spawn(
        Server::builder()
            .add_service(ExternalProcessorServer::new(callouts))
            .serve_with_incoming(UnixListenerStream::new(listener)),
    );
    let channel = Endpoint::from_shared(format!("unix:{}", socket.path.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let requests = phases.into_iter().map(|phase| ProcessingRequest {
        request: Some(phase),
        ..Default::default()
    });
    let mut replies = ExternalProcessorClient::new(channel)
        .process(tokio_stream::iter(requests.collect::<Vec<_>>()))
        .await
        .unwrap()
        .into_inner();
    let mut responses = Vec::new();
    while let Some(reply) = replies.message().await.unwrap() {
        responses.push(reply.response.unwrap());
    }
    responses
}

// One headers message, and the headers it sets and removes.
async fn process(authority: HeaderValue, protected: &str) -> (Vec<(String, String)>, Vec<String>) {
    let phase = Phase::RequestHeaders(HttpHeaders {
        headers: Some(HeaderMap {
            headers: vec![
                authority,
                HeaderValue {
                    key: "x-rail-upstream".to_owned(),
                    raw_value: b"tls".to_vec(),
                    ..Default::default()
                },
            ],
        }),
        ..Default::default()
    });
    let [Response::RequestHeaders(response)] = &send(vec![phase], protected).await[..] else {
        panic!("not one headers response");
    };
    let response = response.clone();
    let common = response.response.unwrap();
    assert!(common.clear_route_cache);
    let mutation = common.header_mutation.unwrap();
    // ext_proc reads `raw_value`.
    assert!(mutation.set_headers.iter().all(|option| {
        option
            .header
            .as_ref()
            .is_some_and(|header| header.value.is_empty())
    }));
    (read_set(&mutation.set_headers), mutation.remove_headers)
}

#[tokio::test]
async fn ext_proc_routes_a_protected_host_from_its_authority() {
    for authority in [
        HeaderValue {
            key: ":authority".to_owned(),
            raw_value: b"gw.internal".to_vec(),
            ..Default::default()
        },
        HeaderValue {
            key: ":authority".to_owned(),
            value: "gw.internal".to_owned(),
            ..Default::default()
        },
    ] {
        let (set, remove) = process(authority, "http://gw.internal:8080").await;

        assert_eq!(
            set,
            pairs(&[
                ("x-rail-upstream", "plain"),
                (":authority", "gw.internal:8080")
            ])
        );
        assert_eq!(remove, ["x-rail", "x-rail-status"]);
    }
}

#[tokio::test]
async fn ext_proc_removes_the_agents_route_to_an_unprotected_host() {
    let authority = HeaderValue {
        key: ":authority".to_owned(),
        raw_value: b"open.example.com".to_vec(),
        ..Default::default()
    };

    let (set, remove) = process(authority, "gw.internal").await;

    assert!(set.is_empty());
    assert_eq!(remove, ["x-rail", "x-rail-status", "x-rail-upstream"]);
}

#[tokio::test]
async fn ext_proc_lets_every_other_phase_through_unchanged() {
    let phases = vec![
        Phase::ResponseHeaders(HttpHeaders::default()),
        Phase::RequestBody(HttpBody::default()),
        Phase::ResponseBody(HttpBody::default()),
        Phase::RequestTrailers(HttpTrailers::default()),
        Phase::ResponseTrailers(HttpTrailers::default()),
    ];

    let responses = send(phases, "mcp.example.com").await;

    assert_eq!(
        responses,
        [
            Response::ResponseHeaders(HeadersResponse::default()),
            Response::RequestBody(BodyResponse::default()),
            Response::ResponseBody(BodyResponse::default()),
            Response::RequestTrailers(TrailersResponse::default()),
            Response::ResponseTrailers(TrailersResponse::default()),
        ]
    );
}
