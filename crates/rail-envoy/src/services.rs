use std::pin::Pin;

use envoy_types::ext_authz::v3::CheckResponseExt;
use envoy_types::ext_authz::v3::pb::{Authorization, CheckRequest, CheckResponse, OkHttpResponse};
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessor;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_response::Response;
use envoy_types::pb::envoy::service::ext_proc::v3::{
    BodyResponse, CommonResponse, HeaderMutation, HeadersResponse, ProcessingRequest,
    ProcessingResponse, TrailersResponse,
};
use tokio_stream::{Stream, StreamExt};
use tonic::{Status, Streaming};

use crate::xrail::XRailHeaders;

/// ext_authz: allows every request, with its x-rail headers.
pub struct Allow;

#[tonic::async_trait]
impl Authorization for Allow {
    async fn check(
        &self,
        request: tonic::Request<CheckRequest>,
    ) -> Result<tonic::Response<CheckResponse>, Status> {
        let http = request
            .get_ref()
            .attributes
            .as_ref()
            .and_then(|attributes| attributes.request.as_ref())
            .and_then(|request| request.http.as_ref());
        // Envoy sends the headers in one of the two, `header_map` when asked for raw headers.
        let sent = http.into_iter().flat_map(|http| {
            http.headers.keys().map(String::as_str).chain(
                http.header_map
                    .iter()
                    .flat_map(|map| map.headers.iter().map(|header| header.key.as_str())),
            )
        });
        let headers = XRailHeaders::of(sent);
        let mut response = CheckResponse::with_status(Status::ok(""));
        response.set_http_response(OkHttpResponse {
            headers_to_remove: headers.remove,
            ..Default::default()
        });
        Ok(tonic::Response::new(response))
    }
}

/// ext_proc: lets every phase continue, with the request's x-rail headers.
pub struct Continue;

type ProcessStream = Pin<Box<dyn Stream<Item = Result<ProcessingResponse, Status>> + Send>>;

#[tonic::async_trait]
impl ExternalProcessor for Continue {
    type ProcessStream = ProcessStream;

    async fn process(
        &self,
        request: tonic::Request<Streaming<ProcessingRequest>>,
    ) -> Result<tonic::Response<ProcessStream>, Status> {
        let replies = request.into_inner().map(|message| {
            let request = message?
                .request
                .ok_or_else(|| Status::invalid_argument("the message names no phase"))?;
            Ok(ProcessingResponse {
                response: Some(continue_after(request)),
                ..Default::default()
            })
        });
        Ok(tonic::Response::new(Box::pin(replies)))
    }
}

// Each phase Envoy sends must be answered with its own kind of response.
fn continue_after(request: Request) -> Response {
    match request {
        Request::RequestHeaders(request) => {
            let sent = request
                .headers
                .iter()
                .flat_map(|map| map.headers.iter().map(|header| header.key.as_str()));
            let headers = XRailHeaders::of(sent);
            Response::RequestHeaders(HeadersResponse {
                response: Some(CommonResponse {
                    header_mutation: Some(HeaderMutation {
                        remove_headers: headers.remove,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            })
        }
        Request::ResponseHeaders(_) => Response::ResponseHeaders(HeadersResponse::default()),
        Request::RequestBody(_) => Response::RequestBody(BodyResponse::default()),
        Request::ResponseBody(_) => Response::ResponseBody(BodyResponse::default()),
        Request::RequestTrailers(_) => Response::RequestTrailers(TrailersResponse::default()),
        Request::ResponseTrailers(_) => Response::ResponseTrailers(TrailersResponse::default()),
    }
}
