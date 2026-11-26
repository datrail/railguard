//! The proxy's callouts handing calls to capture: each reads what Envoy sent it as its mode tells.

use envoy_types::pb::envoy::config::core::v3::HeaderValue;
use envoy_types::pb::envoy::extensions::filters::http::ext_proc::v3::processing_mode::BodySendMode;
use envoy_types::pb::envoy::service::auth::v3::attribute_context::HttpRequest;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request;
use envoy_types::pb::envoy::service::ext_proc::v3::{HttpBody, HttpHeaders, ProtocolConfiguration};
use rail_runtime::{CaptureSender, Ending, HeldRequest};

use crate::headers::{PARTIAL_BODY, value_of};

/// ext_authz's call, handed over if capturing: the body is `raw_body` with `pack_as_bytes`, `body`
/// without.
pub(crate) fn hand_over(sender: &CaptureSender, http: HttpRequest) {
    let partial = http
        .headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_bytes()))
        .chain(
            http.header_map
                .iter()
                .flat_map(|map| &map.headers)
                .map(|header| (header.key.as_str(), value_of(header))),
        )
        .any(|(name, value)| name.eq_ignore_ascii_case(PARTIAL_BODY) && value == b"true");
    let body = if http.raw_body.is_empty() {
        http.body.into_bytes()
    } else {
        http.raw_body
    };
    // A POST sends a body: none, of a size not known to be 0, is one Envoy wasn't asked to send.
    let ending = if body.is_empty() && http.method == "POST" && http.size != 0 {
        Ending::NotSent
    } else if partial {
        Ending::MayBeCut
    } else {
        Ending::Whole
    };
    // Envoy sends `:authority` as `host`, and the headers in one of the two, `header_map` when
    // asked for raw headers.
    let line = [
        (":authority", http.host),
        (":method", http.method),
        (":path", http.path),
    ]
    .map(|(name, value)| (name.to_owned(), value.into_bytes()));
    let sent = http
        .headers
        .into_iter()
        .map(|(name, value)| (name, value.into_bytes()))
        .chain(
            http.header_map
                .into_iter()
                .flat_map(|map| map.headers)
                .map(owned_value),
        );
    if let Some(mut held) = sender.hold(line.into_iter().chain(sent)) {
        held.adopt(body);
        held.finish(ending);
    }
}

// A header's name and value, as `value_of` reads it, without a copy.
fn owned_value(header: HeaderValue) -> (String, Vec<u8>) {
    let value = if header.raw_value.is_empty() {
        header.value.into_bytes()
    } else {
        header.raw_value
    };
    (header.key, value)
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
                    held.finish(Ending::Whole);
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
            held.finish(Ending::Whole);
        } else if matches!(
            self.mode,
            BodySendMode::Buffered | BodySendMode::BufferedPartial
        ) {
            self.held = Some(held);
        } else {
            // `NONE` sends no body; the streamed modes' aren't kept yet.
            held.finish(Ending::NotSent);
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
            Ending::Whole
        } else {
            Ending::MayBeCut
        };
        held.finish(ending);
    }
}
