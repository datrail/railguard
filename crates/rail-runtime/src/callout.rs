//! The settings every host serving a callout reads alike: the port, the largest gRPC message,
//! and the largest body judged.

use rail_policy::sanitize;

use crate::config::{Refused, Settings};

const PORT: &str = "RAIL_CALLOUT_PORT";

const MAX_MESSAGE_MB: &str = "RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB";
// Apigee X takes request bodies up to 10 MB, and gRPC's own default is 4 MB.
const DEFAULT_MAX_MESSAGE_MB: usize = 16;
// High enough that no limit a deployment already sets is refused.
const MAX_MAX_MESSAGE_MB: usize = 100;

const MAX_BODY_MB: &str = "RAIL_MAX_BODY_MB";
const DEFAULT_MAX_BODY_MB: usize = 15;
const MAX_MAX_BODY_MB: usize = 100;

const MB: usize = 1024 * 1024;

/// A limit of the host's own below which the body cap can't be reached, for the startup line.
#[derive(Clone, Copy, Debug)]
pub struct HostLimit {
    pub bytes: usize,
    /// What sets it, e.g. "Apigee's answer limit".
    pub why: &'static str,
}

/// `RAIL_CALLOUT_PORT`, where the server listens, or `default` when unset.
pub fn read_callout_port(
    settings: &mut Settings,
    default: Option<u16>,
) -> Result<Option<u16>, Refused> {
    let port = settings
        .get(PORT)
        .parsed(parse_port)
        .map(|port| port.or(default));
    if let Ok(Some(port)) = &port {
        settings.describe(PORT, port);
    }
    port
}

/// `RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB`, the largest gRPC message received, in bytes.
pub fn read_max_message_bytes(settings: &mut Settings) -> Result<usize, Refused> {
    let mb = settings
        .get(MAX_MESSAGE_MB)
        .parsed(|text| parse_mb(text, MAX_MAX_MESSAGE_MB))
        .map(|mb| mb.unwrap_or(DEFAULT_MAX_MESSAGE_MB));
    if let Ok(mb) = &mb {
        settings.describe(MAX_MESSAGE_MB, mb);
    }
    mb.map(|mb| mb * MB)
}

/// `RAIL_MAX_BODY_MB`, the largest request body judged, in bytes; described with `host_limit`
/// when that's lower, since it's then what refuses first.
pub fn read_max_body_bytes(
    settings: &mut Settings,
    host_limit: Option<HostLimit>,
) -> Result<usize, Refused> {
    let mb = settings
        .get(MAX_BODY_MB)
        .parsed(|text| parse_mb(text, MAX_MAX_BODY_MB))
        .map(|mb| mb.unwrap_or(DEFAULT_MAX_BODY_MB));
    if let Ok(mb) = &mb {
        match host_limit {
            Some(limit) if limit.bytes < mb * MB => settings.describe(
                MAX_BODY_MB,
                format_args!("{mb} ({} bytes in force: {})", limit.bytes, limit.why),
            ),
            _ => settings.describe(MAX_BODY_MB, mb),
        }
    }
    mb.map(|mb| mb * MB)
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

fn parse_mb(text: &str, max: usize) -> Result<usize, String> {
    match text.parse::<usize>() {
        Ok(mb) if (1..=max).contains(&mb) => Ok(mb),
        _ => Err(format!(
            "must be a whole number of MiB, from 1 to {max}, got: {}",
            sanitize(text)
        )),
    }
}
