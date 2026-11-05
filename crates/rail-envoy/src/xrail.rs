use envoy_types::pb::envoy::config::core::v3::header_value_option::HeaderAppendAction;
use envoy_types::pb::envoy::config::core::v3::{HeaderValue, HeaderValueOption};
use rail_runtime::TicketHolder;

/// The ticket, for the gateway to check.
pub const XRAIL: &str = "x-rail";
/// Why there is no ticket, when there is none.
pub const XRAIL_STATUS: &str = "x-rail-status";

/// The x-rail headers one call gets: at most one of the two set, and every other the agent sent
/// removed.
pub struct XRailHeaders {
    pub set: Option<(&'static str, String)>,
    pub remove: Vec<String>,
}

impl XRailHeaders {
    /// From one read of the holder (no ticket and no reason without Rail Center), and the names
    /// of the headers the request carries.
    pub fn of<'a>(tickets: Option<&TicketHolder>, sent: impl IntoIterator<Item = &'a str>) -> Self {
        let set = tickets.map(|tickets| match tickets.current() {
            Ok(ticket) => (XRAIL, ticket.token.expose().to_owned()),
            Err(reason) => (XRAIL_STATUS, reason.as_str().to_owned()),
        });
        let mut remove: Vec<String> = [XRAIL, XRAIL_STATUS]
            .into_iter()
            .map(str::to_owned)
            .chain(
                sent.into_iter()
                    .map(str::to_ascii_lowercase)
                    .filter(|name| is_xrail(name)),
            )
            .filter(|name| set.as_ref().is_none_or(|(set, _)| name != set))
            .collect();
        remove.sort();
        remove.dedup();
        Self { set, remove }
    }

    /// For ext_authz, which reads a header's `value`.
    pub fn options_as_value(&self) -> Vec<HeaderValueOption> {
        self.options(|value| (value.to_owned(), Vec::new()))
    }

    /// For ext_proc, which takes header values as `raw_value`.
    pub fn options_as_raw_value(&self) -> Vec<HeaderValueOption> {
        self.options(|value| (String::new(), value.as_bytes().to_vec()))
    }

    fn options(&self, encode: impl Fn(&str) -> (String, Vec<u8>)) -> Vec<HeaderValueOption> {
        self.set
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
}

// The agent's own x-rail headers never reach the upstream. Takes a lowercased name.
fn is_xrail(name: &str) -> bool {
    name == XRAIL || name.starts_with("x-rail-")
}

#[cfg(test)]
mod tests {
    use super::XRailHeaders;

    #[test]
    fn without_rail_center_every_agent_xrail_header_is_removed() {
        let headers = XRailHeaders::of(None, ["X-Rail", "x-rail-other", "x-railing", "accept"]);

        assert!(headers.set.is_none());
        assert_eq!(headers.remove, ["x-rail", "x-rail-other", "x-rail-status"]);
    }
}
