use std::sync::Arc;

use crate::config::Secret;
use crate::ticket::OpaqueTicket;
use crate::ticket_holder::Unavailable;

/// The ticket, for the gateway to check.
pub const XRAIL: &str = "x-rail";
/// Why there is no ticket, when there is none.
pub const XRAIL_STATUS: &str = "x-rail-status";

/// The proxy's x-rail headers for one call: at most one set, the agent's others removed.
#[derive(Debug, PartialEq, Eq)]
pub struct XRailHeaders {
    /// A secret, since it may be the ticket.
    pub set: Option<(&'static str, Secret)>,
    pub remove: Vec<String>,
}

impl XRailHeaders {
    /// From the holder's reading (`None` without Rail Center) and every header name the host saw.
    pub fn of<'a>(
        ticket: Option<Result<Arc<OpaqueTicket>, Unavailable>>,
        sent: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let set = ticket.map(|ticket| match ticket {
            Ok(ticket) => (XRAIL, ticket.token.clone()),
            Err(reason) => (XRAIL_STATUS, Secret::new(reason.as_str().to_owned())),
        });
        // A header the host never reported can't be removed, so it must report them all.
        let mut remove: Vec<String> = sent
            .into_iter()
            .map(str::to_ascii_lowercase)
            .filter(|name| is_xrail(name))
            // The one set replaces the agent's, whichever order a host applies removals in.
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
