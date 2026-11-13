use std::sync::Arc;

use crate::config::Secret;
use crate::protected_hosts::{ProtectedHost, ProtectedHosts, split_authority};
use crate::ticket::OpaqueTicket;

/// The ticket, for the gateway to check.
pub const XRAIL: &str = "x-rail";
/// Why there is no ticket, when there is none.
pub(crate) const XRAIL_STATUS: &str = "x-rail-status";
/// The route a protected request takes, `tls` or `plain`; Envoy removes it before the upstream.
pub(crate) const XRAIL_UPSTREAM: &str = "x-rail-upstream";
/// Where a protected request connects.
pub(crate) const AUTHORITY: &str = ":authority";

// The agent's port that means none: the default for the plain HTTP it speaks.
const AGENT_DEFAULT_PORT: u16 = 80;

/// The proxy's changes to one request: for a protected host, at most one x-rail header and its
/// route; for any host, the agent's x-rail headers removed.
#[derive(Debug, PartialEq, Eq)]
pub struct XRailHeaders {
    /// For a protected host with Rail Center on.
    pub set: Option<XRailHeader>,
    /// For a protected host.
    pub route: Option<Route>,
    /// `x-rail`, `x-rail-status` and `x-rail-upstream` but those set, and the agent's other x-rail
    /// and `_` headers. Never a header set, whichever order a host applies the two in.
    pub remove: Vec<String>,
}

/// Where a protected request goes.
#[derive(Debug, PartialEq, Eq)]
pub struct Route {
    /// Over HTTPS, unless its entry says `http://`.
    pub tls: bool,
    /// The host and port to connect to, the IPv6 address in brackets.
    pub authority: String,
}

impl Route {
    /// The value of `x-rail-upstream`.
    pub fn upstream(&self) -> &'static str {
        if self.tls { "tls" } else { "plain" }
    }
}

/// Why no ticket is presented, as `x-rail-status` says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// Rail Center answered that this host and sandbox have no ticket.
    NotFound,
    /// The ticket held lapsed with no valid one to replace it.
    Expired,
    /// No answer yet, or the last answer was a failure to reach Rail Center.
    IssuerUnreachable,
}

impl Unavailable {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not-found",
            Self::Expired => "expired",
            Self::IssuerUnreachable => "issuer-unreachable",
        }
    }
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
    /// From the holder's reading (`None` without Rail Center), the request's `:authority`, the
    /// protected hosts, and every header name the host saw.
    pub fn of<'a>(
        ticket: Option<Result<Arc<OpaqueTicket>, Unavailable>>,
        authority: &str,
        protected: &ProtectedHosts,
        sent: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        // An authority that can't be read names no protected host, so it gets no ticket.
        let route = split_authority(authority)
            .ok()
            .and_then(|(host, port)| Some(route_to(protected.get(&host)?, port)));
        let set = ticket
            .filter(|_| route.is_some())
            .map(|ticket| match ticket {
                Ok(ticket) => XRailHeader::Ticket(ticket.token.clone()),
                Err(reason) => XRailHeader::Status(reason.as_str()),
            });
        let is_set = |name: &str| {
            set.as_ref().is_some_and(|set| name == set.name())
                || (route.is_some() && name == XRAIL_UPSTREAM)
        };
        // The gateway's and Envoy's are always removed, others only if the host reports them.
        let mut remove = withheld_headers(
            [XRAIL, XRAIL_STATUS, XRAIL_UPSTREAM]
                .into_iter()
                .chain(sent),
        );
        remove.retain(|name| !is_set(name));
        Self { set, route, remove }
    }

    /// Each header to set, by name, the ticket's value exposed.
    pub fn expose_set(&self) -> Vec<(&'static str, &str)> {
        let route = self.route.iter().flat_map(|route| {
            [
                (XRAIL_UPSTREAM, route.upstream()),
                (AUTHORITY, route.authority.as_str()),
            ]
        });
        self.set
            .iter()
            .map(|set| (set.name(), set.expose_value()))
            .chain(route)
            .collect()
    }
}

// The entry's port, else the agent's unless it's the default, so Envoy picks the scheme's.
fn route_to(entry: &ProtectedHost, agent_port: Option<u16>) -> Route {
    let port = entry
        .port
        .or(agent_port.filter(|&port| port != AGENT_DEFAULT_PORT));
    let host = if entry.host.contains(':') {
        format!("[{}]", entry.host)
    } else {
        entry.host.clone()
    };
    Route {
        tls: entry.tls,
        authority: match port {
            Some(port) => format!("{host}:{port}"),
            None => host,
        },
    }
}

/// The names of `sent` an agent's request must not pass on, lowercased, each once.
pub fn withheld_headers<'a>(sent: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut names: Vec<String> = sent
        .into_iter()
        .map(str::to_ascii_lowercase)
        .filter(|name| is_withheld_header(name))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Whether an agent's header is kept from the upstream: an x-rail header, or any name with `_`.
/// Takes a lowercased name.
pub fn is_withheld_header(name: &str) -> bool {
    // Some upstreams read `_` and `-` alike, so `x_rail` could pass for `x-rail`.
    name == XRAIL || name.starts_with("x-rail-") || name.contains('_')
}
