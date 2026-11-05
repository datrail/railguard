use envoy_types::pb::envoy::config::core::v3::header_value_option::HeaderAppendAction;
use envoy_types::pb::envoy::config::core::v3::{HeaderValue, HeaderValueOption};
use rail_runtime::{Secret, XRailHeaders};

/// For ext_authz, which reads a header's `value`.
pub fn as_value(headers: &XRailHeaders) -> Vec<HeaderValueOption> {
    options(headers, |value| (value.expose().to_owned(), Vec::new()))
}

/// For ext_proc, which takes header values as `raw_value`.
pub fn as_raw_value(headers: &XRailHeaders) -> Vec<HeaderValueOption> {
    options(headers, |value| {
        (String::new(), value.expose().as_bytes().to_vec())
    })
}

fn options(
    headers: &XRailHeaders,
    encode: impl Fn(&Secret) -> (String, Vec<u8>),
) -> Vec<HeaderValueOption> {
    headers
        .set
        .iter()
        .map(|(name, value)| {
            let (value, raw_value) = encode(value);
            HeaderValueOption {
                header: Some(HeaderValue {
                    key: (*name).to_owned(),
                    value,
                    raw_value,
                }),
                append_action: HeaderAppendAction::OverwriteIfExistsOrAdd.into(),
                ..Default::default()
            }
        })
        .collect()
}
