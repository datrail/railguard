use std::fmt;
use std::path::PathBuf;

use rail_runtime::{
    ProtectedHosts, RailCenterSettings, Refused, Settings, TicketSettings, read_callout_port,
    read_disabled, read_log_format, read_log_level, read_protected_hosts, read_rail_center,
    read_ticket_settings,
};

const EXT_SOCKET: &str = "RAIL_EXT_SOCKET";

/// Where the server listens: the socket Envoy shares, or a port, for a host reaching it over the
/// network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listener {
    Unix(PathBuf),
    Tcp(u16),
}

impl fmt::Display for Listener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unix(socket) => write!(f, "{}", socket.display()),
            Self::Tcp(port) => write!(f, "port {port}"),
        }
    }
}

/// What this process runs with.
#[derive(Debug)]
pub struct Config {
    /// The level of this workspace's own log lines.
    pub log_level: tracing::Level,
    pub listener: Listener,
    /// Where this proxy's ticket comes from; `None` when Rail Center is disabled.
    pub ticket: Option<(RailCenterSettings, TicketSettings)>,
    /// The hosts the ticket goes to, and how; routed even when Rail Center is disabled.
    pub protected_hosts: ProtectedHosts,
}

impl Config {
    /// The configuration, every setting judged, or `Err` if anything is refused; `settings` keeps
    /// the refusals.
    pub fn read(settings: &mut Settings) -> Result<Self, Refused> {
        let log_format = read_log_format(settings);
        let log_level = read_log_level(settings);
        let listener = read_listener(settings);
        // Whatever is set is judged either way; only an enabled Rail Center requires its settings.
        let disabled = read_disabled(settings);
        let enabled = disabled == Ok(false);
        let rail_center = read_rail_center(settings, enabled);
        let ticket = read_ticket_settings(settings, enabled);
        let protected_hosts = read_protected_hosts(settings, enabled);
        disabled?;
        log_format?;
        let (log_level, listener, rail_center, ticket, protected_hosts) = (
            log_level?,
            listener?,
            rail_center?,
            ticket?,
            protected_hosts?,
        );
        // Enabled, both are required, so both are Some.
        let ticket = enabled.then(|| {
            rail_center
                .zip(ticket)
                .expect("both are required when Rail Center is used")
        });
        Ok(Self {
            log_level,
            listener,
            ticket,
            protected_hosts,
        })
    }
}

/// `RAIL_EXT_SOCKET` or `RAIL_CALLOUT_PORT`, exactly one, where the server listens: all
/// `rail probe` reads.
// No default: a socket where a port was meant, or the reverse, would serve where nothing calls.
pub fn read_listener(settings: &mut Settings) -> Result<Listener, Refused> {
    let socket = settings.get(EXT_SOCKET).parsed(|text| {
        let path = PathBuf::from(text);
        path.is_absolute()
            .then_some(path)
            .ok_or_else(|| "must be an absolute path".to_owned())
    });
    if let Ok(Some(socket)) = &socket {
        settings.describe(EXT_SOCKET, socket.display());
    }
    let port = read_callout_port(settings, None);
    match (socket?, port?) {
        (Some(socket), None) => Ok(Listener::Unix(socket)),
        (None, Some(port)) => Ok(Listener::Tcp(port)),
        (Some(_), Some(_)) => Err(settings.get(EXT_SOCKET).refuse(
            "and RAIL_CALLOUT_PORT are both set: set only one, the socket or the port to listen on",
        )),
        (None, None) => Err(settings.get(EXT_SOCKET).refuse(
            "or RAIL_CALLOUT_PORT is required, the socket or the port to listen on, and neither is set",
        )),
    }
}
