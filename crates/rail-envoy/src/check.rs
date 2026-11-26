//! ext_authz's request as both roles read it: its header lines, its body, and how the body ended.

use envoy_types::pb::envoy::service::auth::v3::attribute_context::HttpRequest;
use rail_runtime::BodyEnding;

use crate::headers::value_of;

// Set by Envoy when `allow_partial_message` cut the body at `max_request_bytes`.
const PARTIAL_BODY: &str = "x-envoy-auth-partial-body";

/// ext_authz's header lines, in `headers`, or in `header_map` when Envoy is asked for raw headers;
/// then its `:authority` (sent as `host`), `:method` and `:path` fields: the gatherer reads the
/// first, so the lines' own win.
pub(crate) fn check_lines(http: &HttpRequest) -> impl Iterator<Item = (&str, &[u8])> + Clone {
    let fields = [
        (":authority", &http.host),
        (":method", &http.method),
        (":path", &http.path),
    ];
    http.headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_bytes()))
        .chain(
            http.header_map
                .iter()
                .flat_map(|map| &map.headers)
                .map(|header| (header.key.as_str(), value_of(header))),
        )
        .chain(fields.map(|(name, value)| (name, value.as_bytes())))
}

/// ext_authz's body, moved out: `raw_body` with `pack_as_bytes`, `body` without; and how it ended.
pub(crate) fn take_body(http: &mut HttpRequest) -> (Vec<u8>, BodyEnding) {
    let partial = check_lines(http)
        .any(|(name, value)| name.eq_ignore_ascii_case(PARTIAL_BODY) && value == b"true");
    let body = if http.raw_body.is_empty() {
        std::mem::take(&mut http.body).into_bytes()
    } else {
        std::mem::take(&mut http.raw_body)
    };
    // Shorter than its size, it was cut short too.
    let short = usize::try_from(http.size).is_ok_and(|size| body.len() < size);
    // A POST sends a body: none, of a size not known to be 0, is one Envoy wasn't asked to send.
    let ending = if body.is_empty() && http.method == "POST" && http.size != 0 {
        BodyEnding::NotSent
    } else if partial || short {
        BodyEnding::MayBeCut
    } else {
        BodyEnding::Whole
    };
    (body, ending)
}
