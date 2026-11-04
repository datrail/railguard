use std::pin::Pin;
use std::sync::Arc;

use envoy_types::ext_authz::v3::CheckResponseExt;
use envoy_types::ext_authz::v3::pb::{Authorization, CheckRequest, CheckResponse, OkHttpResponse};
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessor;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_response::Response;
use envoy_types::pb::envoy::service::ext_proc::v3::{
    BodyResponse, CommonResponse, HeaderMutation, HeadersResponse, ProcessingRequest,
    ProcessingResponse, TrailersResponse,
};
use rail_runtime::{TicketHolder, XRailHeaders};
use tokio_stream::{Stream, StreamExt};
use tonic::{Status, Streaming};

use crate::xrail::{as_raw_value, as_value};

/// ext_authz: allows every request, with its x-rail headers.
pub struct Allow {
    pub tickets: Option<Arc<TicketHolder>>,
}

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
        let ticket = self.tickets.as_deref().map(TicketHolder::current);
        let headers = XRailHeaders::of(ticket, sent);
        let mut response = CheckResponse::with_status(Status::ok(""));
        response.set_http_response(OkHttpResponse {
            headers: as_value(&headers),
            headers_to_remove: headers.remove,
            ..Default::default()
        });
        Ok(tonic::Response::new(response))
    }
}

/// ext_proc: lets every phase continue, adding the x-rail headers to the request's.
pub struct Continue {
    pub tickets: Option<Arc<TicketHolder>>,
}

type ProcessStream = Pin<Box<dyn Stream<Item = Result<ProcessingResponse, Status>> + Send>>;

#[tonic::async_trait]
impl ExternalProcessor for Continue {
    type ProcessStream = ProcessStream;

    async fn process(
        &self,
        request: tonic::Request<Streaming<ProcessingRequest>>,
    ) -> Result<tonic::Response<ProcessStream>, Status> {
        let tickets = self.tickets.clone();
        let replies = request.into_inner().map(move |message| {
            let request = message?
                .request
                .ok_or_else(|| Status::invalid_argument("the message names no phase"))?;
            Ok(ProcessingResponse {
                response: Some(continue_after(request, tickets.as_deref())),
                ..Default::default()
            })
        });
        Ok(tonic::Response::new(Box::pin(replies)))
    }
}

// Each phase gets its own kind of response (full-duplex bodies, never asked for, would need more).
fn continue_after(request: Request, tickets: Option<&TicketHolder>) -> Response {
    match request {
        Request::RequestHeaders(request) => {
            let sent = request
                .headers
                .iter()
                .flat_map(|map| map.headers.iter().map(|header| header.key.as_str()));
            let headers = XRailHeaders::of(tickets.map(TicketHolder::current), sent);
            Response::RequestHeaders(HeadersResponse {
                response: Some(CommonResponse {
                    header_mutation: Some(HeaderMutation {
                        set_headers: as_raw_value(&headers),
                        remove_headers: headers.remove,
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
