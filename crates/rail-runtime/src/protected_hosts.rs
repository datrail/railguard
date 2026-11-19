use std::collections::BTreeMap;

use rail_model::authority::split_authority;
use rail_model::sanitize::sanitize;

use crate::config::{Refused, Settings};

pub(crate) const PROTECTED_HOSTS: &str = "RAIL_PROXY_PROTECTED_HOSTS";

const ENTRY_FORMAT: &str = "an entry is [http(s)://]host[:port]";
const REQUIRED_BECAUSE: &str =
    "the proxy attaches the agent's identity, so it must name where to send it";

/// One protected host: the host a request is matched on, and where a matched request goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectedHost {
    /// Normalised as a request's `:authority` is: lowercase, no trailing dot, no brackets.
    pub host: String,
    /// HTTPS, unless the entry says `http://`.
    pub tls: bool,
    /// The entry's port; `None` keeps the agent's.
    pub port: Option<u16>,
}

/// The hosts the proxy attaches the ticket for, by normalised host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProtectedHosts(BTreeMap<String, ProtectedHost>);

impl ProtectedHosts {
    /// The entry for `host`, normalised as a request's `:authority` is.
    pub fn get(&self, host: &str) -> Option<&ProtectedHost> {
        self.0.get(host)
    }

    pub fn hosts(&self) -> impl Iterator<Item = &ProtectedHost> {
        self.0.values()
    }
}

/// `RAIL_PROXY_PROTECTED_HOSTS`, judged if set, and required to name a host if `required`.
pub fn read_protected_hosts(
    settings: &mut Settings,
    required: bool,
) -> Result<ProtectedHosts, Refused> {
    let text = settings
        .get(PROTECTED_HOSTS)
        .required_if(required)
        .because(REQUIRED_BECAUSE)
        .text()?;
    // Unset or blank is `None`, so every entry counts, an empty one too.
    let entries: Vec<&str> = text
        .map(|text| text.split(',').map(str::trim).collect())
        .unwrap_or_default();
    let mut hosts = BTreeMap::new();
    // Every entry is judged before any refusal returns.
    let mut refused = None;
    for (index, entry) in entries.iter().enumerate() {
        let position = format!("entry {} of {}", index + 1, entries.len());
        if entry.is_empty() {
            refused = Some(settings.get(PROTECTED_HOSTS).refuse(format_args!(
                "has an empty entry, {position}; {ENTRY_FORMAT}"
            )));
            continue;
        }
        // Not quoted: a credential, query or fragment can hold a secret, which would reach the log.
        let unquoted = if entry.contains('@') {
            Some("a credential")
        } else if entry.contains(['?', '#']) {
            Some("a query or fragment")
        } else {
            None
        };
        if let Some(what) = unquoted {
            refused = Some(settings.get(PROTECTED_HOSTS).refuse(format_args!(
                "has {what} in {position}, which isn't quoted; {ENTRY_FORMAT}"
            )));
            continue;
        }
        match parse_entry(entry) {
            // Matching ignores the scheme and port, so two entries would give one host two routes.
            Ok(host) if hosts.contains_key(&host.host) => {
                refused = Some(settings.get(PROTECTED_HOSTS).refuse(format_args!(
                    "lists {} more than once; one entry per host",
                    sanitize(&host.host)
                )));
            }
            Ok(host) => {
                hosts.insert(host.host.clone(), host);
            }
            Err(reason) => {
                refused = Some(settings.get(PROTECTED_HOSTS).refuse(format_args!(
                    "has an entry that {reason}; {ENTRY_FORMAT}, got: {}",
                    sanitize(entry)
                )));
            }
        }
    }
    if let Some(refused) = refused {
        return Err(refused);
    }
    let hosts = ProtectedHosts(hosts);
    describe(&hosts, settings);
    Ok(hosts)
}

// One entry per host; over plain HTTP, the ticket crosses in the clear.
fn describe(hosts: &ProtectedHosts, out: &mut Settings) {
    if hosts.0.is_empty() {
        out.describe(PROTECTED_HOSTS, "none");
    }
    for host in hosts.hosts() {
        let name = if host.host.contains(':') {
            format!("[{}]", host.host)
        } else {
            host.host.clone()
        };
        let at = host.port.map_or_else(
            || ", at the agent's port".to_owned(),
            |port| format!(":{port}"),
        );
        if host.tls {
            out.describe(PROTECTED_HOSTS, format_args!("{name}{at} (HTTPS)"));
        } else {
            out.warn(
                PROTECTED_HOSTS,
                format_args!("{name}{at} (HTTP)"),
                "the ticket crosses in the clear",
            );
        }
    }
}

// `[http(s)://]host[:port]`, with no credential.
fn parse_entry(entry: &str) -> Result<ProtectedHost, &'static str> {
    let (tls, authority) = match entry.split_once("://") {
        Some((scheme, rest)) => match scheme.to_ascii_lowercase().as_str() {
            "https" => (true, rest),
            "http" => (false, rest),
            _ => return Err("has a scheme other than http:// or https://"),
        },
        None => (true, entry),
    };
    if authority.contains(['/', '?', '#']) {
        return Err("names more than a host: no path, query or fragment");
    }
    let (host, port) = split_authority(authority)?;
    Ok(ProtectedHost { host, tls, port })
}
