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
use rail_runtime::{CaptureSender, ProtectedHosts, Route, TicketHolder, XRailHeader, XRailHeaders};
use tokio_stream::{Stream, StreamExt};
use tonic::{Status, Streaming};

use crate::capture::{StreamCapture, hand_over};
use crate::headers::find_value;
use crate::xrail::{as_raw_value, as_value};

/// Both callouts, which change a request alike: one clone serves each.
#[derive(Clone)]
pub struct Callouts {
    pub tickets: Option<Arc<TicketHolder>>,
    pub protected_hosts: Arc<ProtectedHosts>,
    /// Where calls are handed over, when Rail Center is used.
    pub capture: Option<CaptureSender>,
}

/// ext_authz: allows every request, with its x-rail headers and, to a protected host, its route;
/// with a capture plan held, hands the call to capture.
#[tonic::async_trait]
impl Authorization for Callouts {
    async fn check(
        &self,
        request: tonic::Request<CheckRequest>,
    ) -> Result<tonic::Response<CheckResponse>, Status> {
        let owned = request
            .into_inner()
            .attributes
            .and_then(|attributes| attributes.request)
            .and_then(|request| request.http);
        let http = owned.as_ref();
        // Envoy sends the headers in one of the two, `header_map` when asked for raw headers.
        let sent = http.into_iter().flat_map(|http| {
            http.headers.keys().map(String::as_str).chain(
                http.header_map
                    .iter()
                    .flat_map(|map| map.headers.iter().map(|header| header.key.as_str())),
            )
        });
        // Envoy sends `:authority` as `host`.
        let authority = http.map_or("", |http| http.host.as_str());
        let ticket = self.tickets.as_deref().map(TicketHolder::current);
        let headers = XRailHeaders::of(ticket, authority, &self.protected_hosts, sent);
        log_request("ext_authz", authority, &headers);
        if let (Some(capture), Some(http)) = (&self.capture, owned) {
            hand_over(capture, http);
        }
        let mut response = CheckResponse::with_status(Status::ok(""));
        response.set_http_response(OkHttpResponse {
            headers: as_value(&headers),
            headers_to_remove: headers.remove,
            ..Default::default()
        });
        Ok(tonic::Response::new(response))
    }
}

pub(crate) type ProcessStream =
    Pin<Box<dyn Stream<Item = Result<ProcessingResponse, Status>> + Send>>;

/// ext_proc: lets every phase continue, changing the request's headers as ext_authz does; with a
/// capture plan held at its headers, hands the call to capture once its body is sent, or at its
/// headers when none will be kept.
#[tonic::async_trait]
impl ExternalProcessor for Callouts {
    type ProcessStream = ProcessStream;

    async fn process(
        &self,
        request: tonic::Request<Streaming<ProcessingRequest>>,
    ) -> Result<tonic::Response<ProcessStream>, Status> {
        let tickets = self.tickets.clone();
        let protected_hosts = Arc::clone(&self.protected_hosts);
        let mut capture = self.capture.clone().map(StreamCapture::new);
        let replies = request.into_inner().map(move |message| {
            let message = message?;
            let mut request = message
                .request
                .ok_or_else(|| Status::invalid_argument("the message names no phase"))?;
            if let Some(capture) = &mut capture {
                capture.on(message.protocol_config.as_ref(), &mut request);
            }
            Ok(ProcessingResponse {
                response: Some(continue_after(
                    request,
                    tickets.as_deref(),
                    &protected_hosts,
                )),
                ..Default::default()
            })
        });
        Ok(tonic::Response::new(Box::pin(replies)))
    }
}

// Each phase gets its own kind of response (full-duplex bodies, never asked for, would need more).
fn continue_after(
    request: Request,
    tickets: Option<&TicketHolder>,
    protected_hosts: &ProtectedHosts,
) -> Response {
    match request {
        Request::RequestHeaders(request) => {
            let sent = request.headers.iter().flat_map(|map| &map.headers);
            let authority = find_value(sent.clone(), ":authority")
                .map_or("", |value| std::str::from_utf8(value).unwrap_or_default());
            let headers = XRailHeaders::of(
                tickets.map(TicketHolder::current),
                authority,
                protected_hosts,
                sent.map(|header| header.key.as_str()),
            );
            log_request("ext_proc", authority, &headers);
            Response::RequestHeaders(HeadersResponse {
                response: Some(CommonResponse {
                    header_mutation: Some(HeaderMutation {
                        set_headers: as_raw_value(&headers),
                        remove_headers: headers.remove,
                    }),
                    // The route is picked again, from `x-rail-upstream` as set or removed.
                    clear_route_cache: true,
                    ..Default::default()
                }),
            })
        }
        other => continue_unchanged(other),
    }
}

/// Lets a phase go on unchanged, with the empty response of its kind.
pub(crate) fn continue_unchanged(request: Request) -> Response {
    match request {
        Request::RequestHeaders(_) => Response::RequestHeaders(HeadersResponse::default()),
        Request::ResponseHeaders(_) => Response::ResponseHeaders(HeadersResponse::default()),
        Request::RequestBody(_) => Response::RequestBody(BodyResponse::default()),
        Request::ResponseBody(_) => Response::ResponseBody(BodyResponse::default()),
        Request::RequestTrailers(_) => Response::RequestTrailers(TrailersResponse::default()),
        Request::ResponseTrailers(_) => Response::ResponseTrailers(TrailersResponse::default()),
    }
}

// What a request gets, and never its ticket; one line per request, so at DEBUG.
fn log_request(callout: &'static str, authority: &str, headers: &XRailHeaders) {
    let status = match &headers.set {
        Some(XRailHeader::Status(reason)) => Some(*reason),
        _ => None,
    };
    tracing::debug!(
        callout,
        authority,
        set = headers.set.as_ref().map(XRailHeader::name),
        status,
        upstream = headers.route.as_ref().map(Route::upstream),
        connect = headers.route.as_ref().map(|route| route.authority.as_str()),
        removed = %headers.remove.join(","),
        "request headers decided"
    );
}
