//! The request headers as Envoy sends them to either callout.

use envoy_types::pb::envoy::config::core::v3::HeaderValue;

// Set by Envoy when `allow_partial_message` cut the body at `max_request_bytes`.
pub(crate) const PARTIAL_BODY: &str = "x-envoy-auth-partial-body";

/// A header's value: Envoy sends it in `raw_value`, or in `value` when not asked for raw headers.
pub(crate) fn value_of(header: &HeaderValue) -> &[u8] {
    match header.raw_value.as_slice() {
        [] => header.value.as_bytes(),
        raw => raw,
    }
}

/// The first value of the header `name`, in any letter case. Envoy sends each name read here once:
/// it refuses a repeated `Content-Length` or pseudo-header, and joins two `Host` lines with a comma.
pub(crate) fn find_value<'a>(
    headers: impl IntoIterator<Item = &'a HeaderValue>,
    name: &str,
) -> Option<&'a [u8]> {
    let header = headers
        .into_iter()
        .find(|header| header.key.eq_ignore_ascii_case(name))?;
    Some(value_of(header))
}
