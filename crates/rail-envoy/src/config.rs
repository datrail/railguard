use std::fmt;
use std::path::PathBuf;

use rail_runtime::{
    BundleSettings, ProtectedHosts, RailCenterSettings, Refused, Settings, TicketSettings,
    read_bundle_settings, read_callout_port, read_log_format, read_log_level, read_max_body_bytes,
    read_max_message_bytes, read_protected_hosts, read_rail_center_with, read_ticket_settings,
};

const EXT_SOCKET: &str = "RAIL_EXT_SOCKET";
const MAX_BODY_MB: &str = "RAIL_MAX_BODY_MB";

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

/// What the proxy runs with.
#[derive(Debug)]
pub struct ProxyConfig {
    /// The level of this workspace's own log lines.
    pub log_level: tracing::Level,
    pub listener: Listener,
    /// Where this proxy's ticket comes from; `None` when Rail Center is disabled.
    pub ticket: Option<(RailCenterSettings, TicketSettings)>,
    /// The hosts the ticket goes to, and how; routed even when Rail Center is disabled.
    pub protected_hosts: ProtectedHosts,
}

impl ProxyConfig {
    /// The configuration, every setting judged, or `Err` if anything is refused; `settings` keeps
    /// the refusals.
    pub fn read(settings: &mut Settings) -> Result<Self, Refused> {
        let log_format = read_log_format(settings);
        let log_level = read_log_level(settings);
        let listener = read_listener(settings);
        let (enabled, ticket) = read_rail_center_with(settings, read_ticket_settings);
        let protected_hosts = read_protected_hosts(settings, enabled);
        log_format?;
        let (log_level, listener, ticket, protected_hosts) =
            (log_level?, listener?, ticket?, protected_hosts?);
        Ok(Self {
            log_level,
            listener,
            ticket,
            protected_hosts,
        })
    }
}

/// What the gateway runs with.
#[derive(Debug)]
pub struct GatewayConfig {
    /// The level of this workspace's own log lines.
    pub log_level: tracing::Level,
    pub listener: Listener,
    /// The largest gRPC message received, in bytes.
    pub max_message_bytes: usize,
    /// The largest body judged, in bytes.
    pub max_body_bytes: usize,
    /// Where the bundle and keys come from and denials go; `None` when Rail Center is disabled.
    pub rail_center: Option<(RailCenterSettings, BundleSettings)>,
}

impl GatewayConfig {
    /// The configuration, every setting judged, or `Err` if anything is refused; `settings` keeps
    /// the refusals.
    pub fn read(settings: &mut Settings) -> Result<Self, Refused> {
        let log_format = read_log_format(settings);
        let log_level = read_log_level(settings);
        let listener = read_listener(settings);
        let max_message_bytes = read_max_message_bytes(settings);
        let max_body_bytes = read_max_body_bytes(settings, None);
        // A body Envoy sends whole is one message with its framing, so the cap must be below.
        let reachable = match (&max_body_bytes, &max_message_bytes) {
            (Ok(body), Ok(message)) if body >= message => Err(settings.get(MAX_BODY_MB).refuse(
                "must be below RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB, the largest message read: a body \
                 Envoy buffers arrives as one, with the request's headers",
            )),
            _ => Ok(()),
        };
        let (_, rail_center) = read_rail_center_with(settings, read_bundle_settings);
        log_format?;
        reachable?;
        let (log_level, listener, max_message_bytes, max_body_bytes, rail_center) = (
            log_level?,
            listener?,
            max_message_bytes?,
            max_body_bytes?,
            rail_center?,
        );
        Ok(Self {
            log_level,
            listener,
            max_message_bytes,
            max_body_bytes,
            rail_center,
        })
    }
}

/// `RAIL_EXT_SOCKET` or `RAIL_CALLOUT_PORT`, exactly one, where the server listens: all
/// `rail probe` reads.
// No default: a socket where a port was meant, or the reverse, would serve where nothing calls.
pub(crate) fn read_listener(settings: &mut Settings) -> Result<Listener, Refused> {
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
