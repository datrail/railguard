use std::path::PathBuf;

use rail_runtime::logging::LogFormat;
use rail_runtime::{
    ProtectedHosts, RailCenterSettings, Refused, Settings, TicketSettings, read_disabled,
    read_log_format, read_log_level, read_protected_hosts, read_rail_center, read_ticket_settings,
};

const EXT_SOCKET: &str = "RAIL_EXT_SOCKET";
const DEFAULT_SOCKET: &str = "/run/rail/ext.sock";

/// What this process runs with.
#[derive(Debug)]
pub struct Config {
    /// How each log line is written, as `logging::init` has already read it.
    pub log_format: LogFormat,
    /// The level of this workspace's own log lines.
    pub log_level: tracing::Level,
    pub socket: PathBuf,
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
        let socket = settings
            .get(EXT_SOCKET)
            .parsed(|text| {
                let path = PathBuf::from(text);
                path.is_absolute()
                    .then_some(path)
                    .ok_or_else(|| "must be an absolute path".to_owned())
            })
            .map(|socket| socket.unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET)));
        if let Ok(socket) = &socket {
            settings.describe(EXT_SOCKET, socket.display());
        }
        // Whatever is set is judged either way; only an enabled Rail Center requires its settings.
        let disabled = read_disabled(settings);
        let enabled = disabled == Ok(false);
        let rail_center = read_rail_center(settings, enabled);
        let ticket = read_ticket_settings(settings, enabled);
        let protected_hosts = read_protected_hosts(settings, enabled);
        disabled?;
        let (log_format, log_level, socket, rail_center, ticket, protected_hosts) = (
            log_format?,
            log_level?,
            socket?,
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
            log_format,
            log_level,
            socket,
            ticket,
            protected_hosts,
        })
    }
}
