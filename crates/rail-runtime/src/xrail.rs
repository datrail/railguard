use crate::ticket::OpaqueTicket;
use crate::ticket_holder::Unavailable;

/// The ticket, for the gateway to check.
pub const XRAIL: &str = "x-rail";
/// Why there is no ticket, when there is none.
pub const XRAIL_STATUS: &str = "x-rail-status";

/// The proxy's x-rail headers for one call: at most one set, and every x-rail header the agent
/// sent removed.
#[derive(Debug, PartialEq, Eq)]
pub struct XRailHeaders {
    pub set: Option<(&'static str, String)>,
    pub remove: Vec<String>,
}

impl XRailHeaders {
    /// From the ticket, or why there is none (`None` without Rail Center), and the names of the
    /// headers the request carries.
    pub fn of<'a>(
        ticket: Option<Result<&OpaqueTicket, Unavailable>>,
        sent: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let set = ticket.map(|ticket| match ticket {
            Ok(ticket) => (XRAIL, ticket.token.expose().to_owned()),
            Err(reason) => (XRAIL_STATUS, reason.as_str().to_owned()),
        });
        // The one set replaces the agent's, whichever order a host applies removals in.
        let mut remove: Vec<String> = sent
            .into_iter()
            .map(str::to_ascii_lowercase)
            .filter(|name| is_xrail(name))
            .filter(|name| set.as_ref().is_none_or(|(set, _)| name != set))
            .collect();
        remove.sort();
        remove.dedup();
        Self { set, remove }
    }
}

// The agent's own x-rail headers never reach the upstream. Takes a lowercased name.
fn is_xrail(name: &str) -> bool {
    name == XRAIL || name.starts_with("x-rail-")
}
