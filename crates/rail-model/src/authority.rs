//! The host and port an `:authority` names, spelled one way wherever a host is compared.

use std::net::Ipv6Addr;

const NO_HOST: &str = "names no host";
const EMPTY_PORT: &str = "has an empty or zero port";
const BAD_PORT: &str = "has a port that isn't a number up to 65535";
const NOT_A_HOST: &str = "is not a host name or an IP address";

/// A host and optional port, from an entry or a request's `:authority`: the host lowercased,
/// without one trailing dot or an IPv6 address's brackets. Both go through this, so a host can't
/// be read one way in the setting and another in a request.
pub fn split_authority(authority: &str) -> Result<(String, Option<u16>), &'static str> {
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
