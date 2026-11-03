//! The paths a condition may name, by source.

/// A path under `ticket`, as its keys. Any path is evaluable, and resolves to absent where the
/// payload holds nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct TicketPath(pub Vec<String>);

/// A path under `request`.
#[derive(Clone, Debug, PartialEq)]
pub enum RequestPath {
    Whole,
    EndpointKey,
    Headers,
    /// Matched exactly, so a name with an upper-case letter never exists.
    Header(String),
}

impl RequestPath {
    /// `None` outside what Rail Center publishes: `endpoint_key` and `headers.<name>`.
    pub(crate) fn parse(keys: &[&str]) -> Option<Self> {
        Some(match keys {
            [] => Self::Whole,
            ["endpoint_key"] => Self::EndpointKey,
            ["headers"] => Self::Headers,
            ["headers", name] => Self::Header(name.to_string()),
            _ => return None,
        })
    }
}
