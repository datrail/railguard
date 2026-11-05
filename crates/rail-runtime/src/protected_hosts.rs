use std::collections::BTreeMap;
use std::net::Ipv6Addr;

use rail_policy::sanitize;

use crate::config::Settings;

pub const PROTECTED_HOSTS: &str = "RAIL_PROXY_PROTECTED_HOSTS";

const ENTRY_FORMAT: &str = "an entry is [http(s)://]host[:port]";
const REQUIRED_BECAUSE: &str =
    "the proxy attaches the agent's identity, so it must name where to send it";

const NO_HOST: &str = "names no host";
const EMPTY_PORT: &str = "has an empty or zero port";
const BAD_PORT: &str = "has a port that isn't a number up to 65535";
const NOT_A_HOST: &str = "is not a host name or an IP address";

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
pub fn read_protected_hosts(settings: &mut Settings, required: bool) -> Option<ProtectedHosts> {
    let before = settings.refusals();
    let text = settings
        .get(PROTECTED_HOSTS)
        .required(required)
        .because(REQUIRED_BECAUSE)
        .text();
    // Unset or blank is `None`, so every entry counts, an empty one too.
    let entries: Vec<&str> = text
        .map(|text| text.split(',').map(str::trim).collect())
        .unwrap_or_default();
    let mut hosts = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let position = format!("entry {} of {}", index + 1, entries.len());
        if entry.is_empty() {
            settings.get(PROTECTED_HOSTS).refuse(format_args!(
                "has an empty entry, {position}; {ENTRY_FORMAT}"
            ));
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
            settings.get(PROTECTED_HOSTS).refuse(format_args!(
                "has {what} in {position}, which isn't quoted; {ENTRY_FORMAT}"
            ));
            continue;
        }
        match parse_entry(entry) {
            // Matching ignores the scheme and port, so two entries would give one host two routes.
            Ok(host) if hosts.contains_key(&host.host) => {
                settings.get(PROTECTED_HOSTS).refuse(format_args!(
                    "lists {} more than once; one entry per host",
                    sanitize(&host.host)
                ));
            }
            Ok(host) => {
                hosts.insert(host.host.clone(), host);
            }
            Err(reason) => settings.get(PROTECTED_HOSTS).refuse(format_args!(
                "has an entry that {reason}; {ENTRY_FORMAT}, got: {}",
                sanitize(entry)
            )),
        }
    }
    (settings.refusals() == before).then_some(ProtectedHosts(hosts))
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

/// A host and optional port, from an entry or a request's `:authority`: the host lowercased,
/// without one trailing dot or an IPv6 address's brackets. Both go through this, so a host can't
/// be read one way in the setting and another in a request.
pub(crate) fn split_authority(authority: &str) -> Result<(String, Option<u16>), &'static str> {
    let (host, port, ipv6) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let (address, after) = bracketed.split_once(']').ok_or(NOT_A_HOST)?;
            let port = match after {
                "" => None,
                _ => Some(after.strip_prefix(':').ok_or(NOT_A_HOST)?),
            };
            if address.parse::<Ipv6Addr>().is_err() {
                return Err(NOT_A_HOST);
            }
            (address, port, true)
        }
        None => match authority.split_once(':') {
            Some((host, port)) => (host, Some(port), false),
            None => (authority, None, false),
        },
    };
    let host = host.to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);
    if host.is_empty() {
        return Err(NO_HOST);
    }
    let port = port.map(parse_port).transpose()?;
    if !ipv6 && !is_host_name(host) {
        return Err(NOT_A_HOST);
    }
    Ok((host.to_owned(), port))
}

// Digits only: `u16::from_str` would take a sign.
fn parse_port(text: &str) -> Result<u16, &'static str> {
    if text.is_empty() {
        return Err(EMPTY_PORT);
    }
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(BAD_PORT);
    }
    match text.parse::<u16>() {
        Ok(0) => Err(EMPTY_PORT),
        Ok(port) => Ok(port),
        Err(_) => Err(BAD_PORT),
    }
}

// Dot-separated labels of lowercase letters, digits, `-` and `_`; an IPv4 address is one too.
fn is_host_name(host: &str) -> bool {
    host.split('.').all(|label| {
        !label.is_empty()
            && label.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
    })
}
