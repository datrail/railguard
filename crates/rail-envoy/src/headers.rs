//! The request as Envoy sends it to either callout: its headers, and ext_authz's body.

use envoy_types::pb::envoy::config::core::v3::HeaderValue;
use envoy_types::pb::envoy::service::auth::v3::attribute_context::HttpRequest;
use rail_runtime::BodyEnding;

// Set by Envoy when `allow_partial_message` cut the body at `max_request_bytes`.
const PARTIAL_BODY: &str = "x-envoy-auth-partial-body";

/// ext_authz's header lines, in `headers`, or in `header_map` when Envoy is asked for raw headers;
/// then its `:authority` (sent as `host`), `:method` and `:path` after them, so the lines' own come
/// first.
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

/// A header's value: Envoy sends it in `raw_value`, or in `value` when not asked for raw headers.
pub(crate) fn value_of(header: &HeaderValue) -> &[u8] {
    match header.raw_value.as_slice() {
        [] => header.value.as_bytes(),
        raw => raw,
    }
}

/// The first value of the header `name`, in any letter case. Envoy sends each name read here once:
/// it refuses a repeated pseudo-header.
pub(crate) fn find_value<'a>(
    headers: impl IntoIterator<Item = &'a HeaderValue>,
    name: &str,
) -> Option<&'a [u8]> {
    let header = headers
        .into_iter()
        .find(|header| header.key.eq_ignore_ascii_case(name))?;
    Some(value_of(header))
}
