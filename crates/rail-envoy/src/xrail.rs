use envoy_types::pb::envoy::config::core::v3::header_value_option::HeaderAppendAction;
use envoy_types::pb::envoy::config::core::v3::{HeaderValue, HeaderValueOption};
use rail_runtime::XRailHeaders;

/// For ext_authz, which reads a header's `value`.
pub(crate) fn as_value(headers: &XRailHeaders) -> Vec<HeaderValueOption> {
    let set = headers.expose_set().into_iter();
    set.map(|(name, value)| value_option(name, value)).collect()
}

/// For ext_proc, which takes header values as `raw_value`.
pub(crate) fn as_raw_value(headers: &XRailHeaders) -> Vec<HeaderValueOption> {
    let set = headers.expose_set().into_iter();
    set.map(|(name, value)| raw_value_option(name, value))
        .collect()
}

/// One header set, for ext_authz.
pub(crate) fn value_option(name: &str, value: &str) -> HeaderValueOption {
    option(name, value.to_owned(), Vec::new())
}

/// One header set, for ext_proc.
pub(crate) fn raw_value_option(name: &str, value: &str) -> HeaderValueOption {
    option(name, String::new(), value.as_bytes().to_vec())
}

fn option(name: &str, value: String, raw_value: Vec<u8>) -> HeaderValueOption {
    HeaderValueOption {
        header: Some(HeaderValue {
            key: name.to_owned(),
            value,
            raw_value,
        }),
        append_action: HeaderAppendAction::OverwriteIfExistsOrAdd.into(),
        ..Default::default()
    }
}
