use rail_policy::sanitize;
use rail_runtime::{
    BundleSettings, RailCenterSettings, Refused, Settings, read_bundle_settings, read_disabled,
    read_rail_center,
};

const PORT: &str = "RAIL_GATEWAY_PORT";
const DEFAULT_PORT: u16 = 8080;

const MAX_MESSAGE_MB: &str = "RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB";
// Apigee X takes request bodies up to 10 MB, and gRPC's own default is 4 MB.
const DEFAULT_MAX_MESSAGE_MB: usize = 16;
// As Python's, so no setting it took is refused.
const MAX_MAX_MESSAGE_MB: usize = 100;
const MB: usize = 1024 * 1024;

/// What this process runs with.
#[derive(Debug)]
pub struct Config {
    pub port: u16,
    /// The largest gRPC message received, in bytes.
    pub max_message_bytes: usize,
    /// Where the bundle and keys come from and denials go; `None` when Rail Center is disabled.
    pub rail_center: Option<(RailCenterSettings, BundleSettings)>,
}

impl Config {
    /// The configuration, every setting judged, or `Err` if anything is refused; `settings` keeps
    /// the refusals.
    pub fn read(settings: &mut Settings) -> Result<Self, Refused> {
        let port = settings
            .get(PORT)
            .parsed(parse_port)
            .map(|port| port.unwrap_or(DEFAULT_PORT));
        if let Ok(port) = &port {
            settings.describe(PORT, port);
        }
        let max_message_mb = settings
            .get(MAX_MESSAGE_MB)
            .parsed(parse_max_message_mb)
            .map(|mb| mb.unwrap_or(DEFAULT_MAX_MESSAGE_MB));
        if let Ok(mb) = &max_message_mb {
            settings.describe(MAX_MESSAGE_MB, mb);
        }
        // Whatever is set is judged either way; only an enabled Rail Center requires its settings.
        let disabled = read_disabled(settings);
        let enabled = disabled == Ok(false);
        let rail_center = read_rail_center(settings, enabled);
        let bundle = read_bundle_settings(settings, enabled);
        disabled?;
        let (port, max_message_mb, rail_center, bundle) =
            (port?, max_message_mb?, rail_center?, bundle?);
        // Enabled, both are required, so both are Some.
        let rail_center = enabled.then(|| {
            rail_center
                .zip(bundle)
                .expect("both are required when Rail Center is used")
        });
        Ok(Self {
            port,
            max_message_bytes: max_message_mb * MB,
            rail_center,
        })
    }
}

fn parse_port(text: &str) -> Result<u16, String> {
    match text.parse::<u16>() {
        Ok(port) if port != 0 => Ok(port),
        _ => Err(format!(
            "must be a port, from 1 to 65535, got: {}",
            sanitize(text)
        )),
    }
}

fn parse_max_message_mb(text: &str) -> Result<usize, String> {
    match text.parse::<usize>() {
        Ok(mb) if (1..=MAX_MAX_MESSAGE_MB).contains(&mb) => Ok(mb),
        _ => Err(format!(
            "must be a whole number of MiB, from 1 to {MAX_MAX_MESSAGE_MB}, got: {}",
            sanitize(text)
        )),
    }
}
