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
    pub set: Option<XRailHeader>,
    /// `x-rail` and `x-rail-status` but the one set, and the agent's other x-rail and `_` headers.
    pub remove: Vec<String>,
}

/// The one x-rail header the proxy sets.
#[derive(Debug, PartialEq, Eq)]
pub enum XRailHeader {
    /// The ticket, in `x-rail`; kept out of Debug.
    Ticket(Secret),
    /// Why there is no ticket, in `x-rail-status`; not a secret.
    Status(&'static str),
}

impl XRailHeader {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Ticket(_) => XRAIL,
            Self::Status(_) => XRAIL_STATUS,
        }
    }

    pub fn expose_value(&self) -> &str {
        match self {
            Self::Ticket(token) => token.expose(),
            Self::Status(reason) => reason,
        }
    }
}

impl XRailHeaders {
    /// From the holder's reading (`None` without Rail Center) and every header name the host saw.
    pub fn of<'a>(
        ticket: Option<Result<Arc<OpaqueTicket>, Unavailable>>,
        sent: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let set = ticket.map(|ticket| match ticket {
            Ok(ticket) => XRailHeader::Ticket(ticket.token.clone()),
            Err(reason) => XRailHeader::Status(reason.as_str()),
        });
        // The two the gateway reads are always removed, others only if the host reports them.
        let mut remove: Vec<String> = [XRAIL, XRAIL_STATUS]
            .into_iter()
            .map(str::to_owned)
            .chain(
                sent.into_iter()
                    .map(str::to_ascii_lowercase)
                    .filter(|name| !may_cross(name)),
            )
            // The one set replaces the agent's, whichever order a host applies removals in.
            .filter(|name| set.as_ref().is_none_or(|set| name != set.name()))
            .collect();
        remove.sort();
        remove.dedup();
        Self { set, remove }
    }
}

// No agent x-rail header, nor any name with `_`, reaches the upstream. Takes a lowercased name.
fn may_cross(name: &str) -> bool {
    // Some upstreams read `_` and `-` alike, so `x_rail` could pass for `x-rail`.
    !(name == XRAIL || name.starts_with("x-rail-") || name.contains('_'))
}
