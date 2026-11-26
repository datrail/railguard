//! What an agent's request is read as by every use.

use percent_encoding::percent_decode_str;

/// The ticket, for the gateway to check.
pub const XRAIL: &str = "x-rail";

/// Whether an agent's header is kept from the upstream: an x-rail header, or any name with `_`.
/// Takes a lowercased name.
pub fn is_withheld_header(name: &str) -> bool {
    // Some upstreams read `_` and `-` alike, so `x_rail` could pass for `x-rail`.
    name == XRAIL || name.starts_with("x-rail-") || name.contains('_')
}

/// The path of `uri`, without the query, percent-decoded: an invalid escape is kept, and invalid
/// UTF-8 replaced.
pub fn read_path(uri: &str) -> String {
    let path = uri.split_once('?').map_or(uri, |(path, _)| path);
    let path = percent_decode_str(path).decode_utf8_lossy();
    if path.is_empty() {
        "/".to_owned()
    } else {
        path.into_owned()
    }
}
