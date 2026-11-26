use std::pin::Pin;
use std::sync::Arc;

use envoy_types::ext_authz::v3::CheckResponseExt;
use envoy_types::ext_authz::v3::pb::{Authorization, CheckRequest, CheckResponse, OkHttpResponse};
use envoy_types::pb::envoy::extensions::filters::http::ext_proc::v3::processing_mode::BodySendMode;
use envoy_types::pb::envoy::service::auth::v3::attribute_context::HttpRequest;
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessor;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_response::Response;
use envoy_types::pb::envoy::service::ext_proc::v3::{
    BodyResponse, CommonResponse, HeaderMutation, HeadersResponse, HttpBody, HttpHeaders,
    ProcessingRequest, ProcessingResponse, ProtocolConfiguration, TrailersResponse,
};
use rail_runtime::{
    CaptureSender, ProtectedHosts, Route, SentBody, SentRequest, TicketHolder, XRailHeader,
    XRailHeaders,
};
use tokio_stream::{Stream, StreamExt};
use tonic::{Status, Streaming};

use crate::headers::{find_value, read_length};
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
        // Envoy sends `:authority` as `host`.
        let authority = http.map_or("", |http| http.host.as_str());
        let ticket = self.tickets.as_deref().map(TicketHolder::current);
        let headers = XRailHeaders::of(ticket, authority, &self.protected_hosts, sent);
        log_request("ext_authz", authority, &headers);
        if let (Some(capture), Some(http)) = (&self.capture, http)
            && capture.is_capturing()
        {
            capture.send_request(observe(http));
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

// The call as Envoy sent it: the body is `raw_body` with `pack_as_bytes`, `body` without. Envoy's
// `size` and partial-body header tell the bytes sent, not the body's, once it's cut short.
fn observe(http: &HttpRequest) -> SentRequest<'_> {
    let sent = if http.raw_body.is_empty() {
        http.body.as_bytes()
    } else {
        &http.raw_body
    };
    let declared = declared_length(http);
    // A POST sends a body: none, of a size not known to be 0, is one Envoy wasn't asked to send.
    let unsent = sent.is_empty() && http.method == "POST" && http.size != 0 && declared != Some(0);
    let body = if unsent {
        SentBody::NotSent { declared }
    } else {
        SentBody::Sent {
            bytes: sent,
            declared,
        }
    };
    SentRequest {
        authority: http.host.as_bytes(),
        method: http.method.as_bytes(),
        path: http.path.as_bytes(),
        body,
    }
}

// `Content-Length` as sent, from either header field.
fn declared_length(http: &HttpRequest) -> Option<u64> {
    let header_map = http.header_map.iter().flat_map(|map| &map.headers);
    find_value(header_map, "content-length")
        .or_else(|| http.headers.get("content-length").map(String::as_bytes))
        .and_then(read_length)
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
            let request = message
                .request
                .ok_or_else(|| Status::invalid_argument("the message names no phase"))?;
            if let Some(capture) = &mut capture {
                capture.on(message.protocol_config.as_ref(), &request);
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

// One ext_proc stream's call for capture, held from its headers until its body is sent, or handed
// over as cut short when the stream ends first.
struct StreamCapture {
    sender: CaptureSender,
    // Sent in the stream's first message; none is `NONE`, Envoy's default.
    mode: BodySendMode,
    held: Option<HeldCall>,
}

// A call's request line and declared length, read at its headers.
struct HeldCall {
    authority: Vec<u8>,
    method: Vec<u8>,
    path: Vec<u8>,
    declared: Option<u64>,
}

impl StreamCapture {
    fn new(sender: CaptureSender) -> Self {
        Self {
            sender,
            mode: BodySendMode::None,
            held: None,
        }
    }

    fn on(&mut self, config: Option<&ProtocolConfiguration>, request: &Request) {
        if let Some(config) = config {
            self.mode = config.request_body_mode();
        }
        match request {
            Request::RequestHeaders(headers) => self.on_headers(headers),
            Request::RequestBody(body) => self.on_body(body),
            Request::RequestTrailers(_) => self.on_trailers(),
            _ => {}
        }
    }

    fn on_headers(&mut self, headers: &HttpHeaders) {
        if !self.sender.is_capturing() {
            return;
        }
        let call = HeldCall::read(headers);
        if headers.end_of_stream {
            let empty = SentBody::Sent {
                bytes: &[],
                declared: call.declared,
            };
            call.send(&self.sender, empty);
        } else if matches!(
            self.mode,
            BodySendMode::Buffered | BodySendMode::BufferedPartial
        ) {
            self.held = Some(call);
        } else {
            // `NONE` sends no body; the streamed modes' aren't kept yet.
            let unsent = SentBody::NotSent {
                declared: call.declared,
            };
            call.send(&self.sender, unsent);
        }
    }

    // Buffered, the body is one message: `BUFFERED` refuses one past its limit, `BUFFERED_PARTIAL`
    // stops there, not ending the stream, as trailers following would.
    fn on_body(&mut self, body: &HttpBody) {
        let Some(call) = self.held.take() else {
            return;
        };
        let declared = call.declared;
        let whole = body.end_of_stream || self.mode == BodySendMode::Buffered;
        // A `Content-Length` tells a body cut short itself; without one, the stream not ending does.
        let sent = if whole || declared.is_some() {
            SentBody::Sent {
                bytes: &body.body,
                declared,
            }
        } else {
            SentBody::CutShort {
                sent_bytes: body.body.len() as u64,
                declared,
            }
        };
        call.send(&self.sender, sent);
    }

    // Trailers with the call still held: no body message came, so the body was empty.
    fn on_trailers(&mut self) {
        if let Some(call) = self.held.take() {
            let empty = SentBody::Sent {
                bytes: &[],
                declared: call.declared,
            };
            call.send(&self.sender, empty);
        }
    }
}

impl Drop for StreamCapture {
    fn drop(&mut self) {
        if let Some(call) = self.held.take() {
            let ended = SentBody::CutShort {
                sent_bytes: 0,
                declared: call.declared,
            };
            call.send(&self.sender, ended);
        }
    }
}

impl HeldCall {
    fn read(headers: &HttpHeaders) -> Self {
        let sent = headers.headers.iter().flat_map(|map| &map.headers);
        let find = |name| find_value(sent.clone(), name).unwrap_or_default().to_vec();
        Self {
            authority: find(":authority"),
            method: find(":method"),
            path: find(":path"),
            declared: find_value(sent, "content-length").and_then(read_length),
        }
    }

    fn send(&self, sender: &CaptureSender, body: SentBody<'_>) {
        sender.send_request(SentRequest {
            authority: &self.authority,
            method: &self.method,
            path: &self.path,
            body,
        });
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
