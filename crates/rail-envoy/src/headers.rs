//! The request headers as Envoy sends them to either callout.

use envoy_types::pb::envoy::config::core::v3::HeaderValue;
use envoy_types::pb::envoy::service::ext_proc::v3::HttpHeaders;

/// ext_proc's header lines, as sent in its headers message.
pub(crate) fn process_lines(headers: &HttpHeaders) -> impl Iterator<Item = (&str, &[u8])> + Clone {
    headers
        .headers
        .iter()
        .flat_map(|map| &map.headers)
        .map(|header| (header.key.as_str(), value_of(header)))
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
