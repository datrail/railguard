/// The ticket, for the gateway to check.
pub const XRAIL: &str = "x-rail";
/// Why there is no ticket, when there is none.
pub const XRAIL_STATUS: &str = "x-rail-status";

/// The x-rail headers one call gets: every one the agent sent removed.
pub struct XRailHeaders {
    pub remove: Vec<String>,
}

impl XRailHeaders {
    /// From the names of the headers the request carries.
    pub fn of<'a>(sent: impl IntoIterator<Item = &'a str>) -> Self {
        let mut remove: Vec<String> = [XRAIL, XRAIL_STATUS]
            .into_iter()
            .chain(sent.into_iter().filter(|name| is_xrail(name)))
            .map(str::to_ascii_lowercase)
            .collect();
        remove.sort();
        remove.dedup();
        Self { remove }
    }
}

// The agent's own x-rail headers never reach the upstream.
fn is_xrail(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == XRAIL || name.starts_with("x-rail-")
}

#[cfg(test)]
mod tests {
    use super::XRailHeaders;

    #[test]
    fn every_agent_xrail_header_is_removed() {
        let headers = XRailHeaders::of(["X-Rail", "x-rail-other", "x-railing", "accept"]);

        assert_eq!(headers.remove, ["x-rail", "x-rail-other", "x-rail-status"]);
    }
}
