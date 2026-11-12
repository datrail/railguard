use rail_runtime::{
    BundleSettings, HostLimit, RailCenterSettings, Refused, Settings, read_bundle_settings,
    read_callout_port, read_disabled, read_log_format, read_log_level, read_max_body_bytes,
    read_max_message_bytes, read_rail_center,
};

use crate::callout::MAX_ANSWERED_BYTES;

// The callout's settings are named for any host serving a callout, not the gateway's alone;
// the retired names are refused, so a deployment still setting one can't quietly lose it.
const PORT: &str = "RAIL_CALLOUT_PORT";
const RETIRED_PORT: &str = "RAIL_GATEWAY_PORT";
const DEFAULT_PORT: u16 = 8080;

const MAX_MESSAGE_MB: &str = "RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB";
const RETIRED_MAX_MESSAGE_MB: &str = "RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB";

// The answer echoes the request, so Apigee's 4 MiB answer limit can refuse before the cap.
const APIGEE_LIMIT: HostLimit = HostLimit {
    bytes: MAX_ANSWERED_BYTES,
    why: "Apigee's answer limit, on the whole request",
};

/// What this process runs with.
#[derive(Debug)]
pub struct Config {
    /// The level of this workspace's own log lines.
    pub log_level: tracing::Level,
    pub port: u16,
    /// The largest gRPC message received, in bytes.
    pub max_message_bytes: usize,
    /// The largest body judged, in bytes; Apigee's answer limit refuses first when lower.
    pub max_body_bytes: usize,
    /// Where the bundle and keys come from and denials go; `None` when Rail Center is disabled.
    pub rail_center: Option<(RailCenterSettings, BundleSettings)>,
}

impl Config {
    /// The configuration, every setting judged, or `Err` if anything is refused; `settings` keeps
    /// the refusals.
    pub fn read(settings: &mut Settings) -> Result<Self, Refused> {
        let log_format = read_log_format(settings);
        let log_level = read_log_level(settings);
        let retired = [
            settings
                .get(RETIRED_PORT)
                .retired(&format!("{PORT} sets the port")),
            settings
                .get(RETIRED_MAX_MESSAGE_MB)
                .retired(&format!("{MAX_MESSAGE_MB} sets the limit")),
        ];
        let port = read_port(settings);
        let max_message_bytes = read_max_message_bytes(settings);
        let max_body_bytes = read_max_body_bytes(settings, Some(APIGEE_LIMIT));
        // Whatever is set is judged either way; only an enabled Rail Center requires its settings.
        let disabled = read_disabled(settings);
        let enabled = disabled == Ok(false);
        let rail_center = read_rail_center(settings, enabled);
        let bundle = read_bundle_settings(settings, enabled);
        disabled?;
        retired.into_iter().try_for_each(|retired| retired)?;
        log_format?;
        let (log_level, port, max_message_bytes, max_body_bytes, rail_center, bundle) = (
            log_level?,
            port?,
            max_message_bytes?,
            max_body_bytes?,
            rail_center?,
            bundle?,
        );
        // Enabled, both are required, so both are Some.
        let rail_center = enabled.then(|| {
            rail_center
                .zip(bundle)
                .expect("both are required when Rail Center is used")
        });
        Ok(Self {
            log_level,
            port,
            max_message_bytes,
            max_body_bytes,
            rail_center,
        })
    }
}

/// `RAIL_CALLOUT_PORT`, where the server listens: all `rail probe` reads.
pub fn read_port(settings: &mut Settings) -> Result<u16, Refused> {
    read_callout_port(settings, Some(DEFAULT_PORT)).map(|port| port.expect("a default is given"))
}
