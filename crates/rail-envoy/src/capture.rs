//! The proxy's callouts handing calls to capture: each reads what Envoy sent it as its mode tells.

use envoy_types::pb::envoy::extensions::filters::http::ext_proc::v3::processing_mode::BodySendMode;
use envoy_types::pb::envoy::service::auth::v3::attribute_context::HttpRequest;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request;
use envoy_types::pb::envoy::service::ext_proc::v3::{HttpBody, HttpHeaders, ProtocolConfiguration};
use rail_runtime::{BodyEnding, CaptureSender, HeldRequest};

use crate::headers::{check_lines, take_body, value_of};

/// ext_authz's call, handed over if capturing.
pub(crate) fn hand_over(sender: &CaptureSender, mut http: HttpRequest) {
    let Some(mut held) = sender.hold(check_lines(&http)) else {
        return;
    };
    let (body, ending) = take_body(&mut http);
    held.adopt(body);
    held.finish(ending);
}

/// One ext_proc stream's call for capture, held from its headers until its body ends.
pub(crate) struct StreamCapture {
    sender: CaptureSender,
    // Sent in the stream's first message; none is `NONE`, Envoy's default.
    mode: BodySendMode,
    held: Option<HeldRequest>,
}

impl StreamCapture {
    pub(crate) fn new(sender: CaptureSender) -> Self {
        Self {
            sender,
            mode: BodySendMode::None,
            held: None,
        }
    }

    /// Reads `request`, taking a body it holds.
    pub(crate) fn on(&mut self, config: Option<&ProtocolConfiguration>, request: &mut Request) {
        if let Some(config) = config {
            self.mode = config.request_body_mode();
        }
        match request {
            Request::RequestHeaders(headers) => self.on_headers(headers),
            Request::RequestBody(body) => self.on_body(body),
            // Trailers with the call still held: no body message came, so the body was empty.
            Request::RequestTrailers(_) => {
                if let Some(held) = self.held.take() {
                    held.finish(BodyEnding::Whole);
                }
            }
            _ => {}
        }
    }

    fn on_headers(&mut self, headers: &HttpHeaders) {
        let sent = headers.headers.iter().flat_map(|map| &map.headers);
        let Some(held) = self
            .sender
            .hold(sent.map(|header| (header.key.as_bytes(), value_of(header))))
        else {
            return;
        };
        if headers.end_of_stream {
            held.finish(BodyEnding::Whole);
        } else if matches!(
            self.mode,
            BodySendMode::Buffered | BodySendMode::BufferedPartial
        ) {
            self.held = Some(held);
        } else {
            // `NONE` sends no body; the streamed modes' aren't kept yet.
            held.finish(BodyEnding::NotSent);
        }
    }

    // Buffered, the body is one message: `BUFFERED` refuses one past its limit, `BUFFERED_PARTIAL`
    // stops there, not ending the stream, as trailers following would.
    fn on_body(&mut self, body: &mut HttpBody) {
        let Some(mut held) = self.held.take() else {
            return;
        };
        held.adopt(std::mem::take(&mut body.body));
        let ending = if body.end_of_stream || self.mode == BodySendMode::Buffered {
            BodyEnding::Whole
        } else {
            BodyEnding::MayBeCut
        };
        held.finish(ending);
    }
}
