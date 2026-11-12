use std::path::PathBuf;
use std::time::Duration;

use rail_envoy::{GatewayConfig, Listener, ProxyConfig};
use rail_runtime::{Level, Source, Startup};

const TOKEN: &str = "rc_svc_abcdefghijklmnopqrstuvwxyzABCDEFGHIJK-_0123";

// Every setting, set.
const COMPLETE: [(&str, &str); 10] = [
    ("RAIL_EXT_SOCKET", "/tmp/rail/ext.sock"),
    ("RAIL_DISABLED", "false"),
    ("RAIL_CENTER_URL", "http://rail-center.internal:8080"),
    ("RAIL_CENTER_ALLOW_HTTP", "true"),
    ("RAIL_AUTH_MODE", "bearer"),
    ("RAIL_AUTH_TOKEN", TOKEN),
    ("RAIL_HOST_ID", "host-1"),
    ("RAIL_SANDBOX_NAME", "sandbox-1"),
    ("RAIL_TICKET_REFRESH_SECONDS", "600"),
    (
        "RAIL_PROXY_PROTECTED_HOSTS",
        "mcp.example.com:8443,http://gw.internal",
    ),
];

const ENABLED: [(&str, &str); 5] = [
    ("RAIL_CENTER_URL", "https://rail-center.example"),
    ("RAIL_PROXY_PROTECTED_HOSTS", "mcp.example.com:8443"),
    ("RAIL_HOST_ID", "host-1"),
    ("RAIL_SANDBOX_NAME", "sandbox-1"),
    ("RAIL_TICKET_REFRESH_SECONDS", "600"),
];

// Where a case names no listener, the socket: one is required.
fn listening<'a>(pairs: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut pairs = pairs.to_vec();
    let names_one = pairs
        .iter()
        .any(|(name, _)| ["RAIL_EXT_SOCKET", "RAIL_CALLOUT_PORT"].contains(name));
    if !names_one {
        pairs.insert(0, ("RAIL_EXT_SOCKET", "/run/rail/ext.sock"));
    }
    pairs
}

fn read_startup(pairs: &[(&str, &str)]) -> Result<(ProxyConfig, Startup), Vec<String>> {
    rail_test_support::read(&listening(pairs), ProxyConfig::read)
}

fn describe(pairs: &[(&str, &str)]) -> Vec<(&'static str, String, Level)> {
    rail_test_support::describe(&listening(pairs), ProxyConfig::read)
}

fn read(pairs: &[(&str, &str)]) -> Result<ProxyConfig, Vec<String>> {
    read_startup(pairs).map(|(config, _)| config)
}

fn assert_no_token(entries: &[(&'static str, String, Level)]) {
    let secret = TOKEN.strip_prefix("rc_svc_").unwrap();
    for (name, value, _) in entries {
        assert!(!value.contains(secret), "{name} shows the token");
    }
}

#[test]
fn disabled_describes_every_setting_set_though_its_group_is_incomplete() {
    let pairs = [
        ("RAIL_DISABLED", "true"),
        ("RAIL_TICKET_REFRESH_SECONDS", "600"),
        ("RAIL_CENTER_ALLOW_HTTP", "true"),
        ("RAIL_AUTH_MODE", "bearer"),
        ("RAIL_AUTH_TOKEN", TOKEN),
    ];

    let entries = describe(&pairs);

    let entries_of = |name| entries.iter().filter(move |(named, ..)| *named == name);
    for (name, _) in pairs {
        assert_eq!(entries_of(name).count(), 1, "{name}");
    }
    assert_eq!(
        entries,
        [
            ("RAIL_LOG_FORMAT", "json".to_owned(), Level::Info),
            ("RAIL_LOG_LEVEL", "info".to_owned(), Level::Info),
            (
                "RAIL_EXT_SOCKET",
                "/run/rail/ext.sock".to_owned(),
                Level::Info
            ),
            (
                "RAIL_DISABLED",
                "true".to_owned(),
                Level::Warn("Rail Center isn't used: nothing is fetched from it")
            ),
            ("RAIL_PROXY_PROTECTED_HOSTS", "none".to_owned(), Level::Info),
            ("RAIL_AUTH_MODE", "bearer".to_owned(), Level::Info),
            ("RAIL_AUTH_TOKEN", "set".to_owned(), Level::Info),
            ("RAIL_CENTER_ALLOW_HTTP", "true".to_owned(), Level::Info),
            ("RAIL_TICKET_REFRESH_SECONDS", "600".to_owned(), Level::Info),
        ]
    );
    assert_no_token(&entries);
}

#[test]
fn without_a_credential_the_token_is_described_as_unset() {
    let pairs: Vec<_> = COMPLETE
        .into_iter()
        .filter_map(|(name, value)| match name {
            "RAIL_AUTH_MODE" => Some((name, "none")),
            "RAIL_AUTH_TOKEN" => None,
            _ => Some((name, value)),
        })
        .collect();

    let entries = describe(&pairs);

    assert_eq!(
        entries[6..8],
        [
            ("RAIL_AUTH_MODE", "none".to_owned(), Level::Info),
            ("RAIL_AUTH_TOKEN", "unset".to_owned(), Level::Info),
        ]
    );
}

#[test]
fn the_description_says_each_value_as_used_and_never_the_token() {
    let described = describe(&COMPLETE);

    let entries: Vec<_> = described
        .iter()
        .map(|(name, value, level)| (*name, value.as_str(), *level))
        .collect();
    assert_eq!(
        entries,
        [
            ("RAIL_LOG_FORMAT", "json", Level::Info),
            ("RAIL_LOG_LEVEL", "info", Level::Info),
            ("RAIL_EXT_SOCKET", "/tmp/rail/ext.sock", Level::Info),
            ("RAIL_DISABLED", "false", Level::Info),
            (
                "RAIL_CENTER_URL",
                "http://rail-center.internal:8080",
                Level::Warn("the credential and what Rail Center returns cross in the clear")
            ),
            ("RAIL_CENTER_ALLOW_HTTP", "true", Level::Info),
            ("RAIL_AUTH_MODE", "bearer", Level::Info),
            ("RAIL_AUTH_TOKEN", "set", Level::Info),
            ("RAIL_HOST_ID", "host-1", Level::Info),
            ("RAIL_SANDBOX_NAME", "sandbox-1", Level::Info),
            ("RAIL_TICKET_REFRESH_SECONDS", "600", Level::Info),
            (
                "RAIL_PROXY_PROTECTED_HOSTS",
                "gw.internal, at the agent's port (HTTP)",
                Level::Warn("the ticket crosses in the clear")
            ),
            (
                "RAIL_PROXY_PROTECTED_HOSTS",
                "mcp.example.com:8443 (HTTPS)",
                Level::Info
            ),
        ]
    );
    assert_no_token(&described);
}

#[test]
fn the_defaults_are_described_as_used_and_disabled_as_a_warning() {
    let (_, startup) = read_startup(&[("RAIL_DISABLED", "true")]).unwrap();

    let entries: Vec<_> = startup
        .entries
        .iter()
        .map(|entry| (entry.name, entry.value.as_str(), entry.level, entry.source))
        .collect();
    assert_eq!(
        entries,
        [
            ("RAIL_LOG_FORMAT", "json", Level::Info, Source::Default),
            ("RAIL_LOG_LEVEL", "info", Level::Info, Source::Default),
            (
                "RAIL_EXT_SOCKET",
                "/run/rail/ext.sock",
                Level::Info,
                Source::Env
            ),
            (
                "RAIL_DISABLED",
                "true",
                Level::Warn("Rail Center isn't used: nothing is fetched from it"),
                Source::Env
            ),
            (
                "RAIL_PROXY_PROTECTED_HOSTS",
                "none",
                Level::Info,
                Source::Default
            ),
        ]
    );
}

#[test]
fn a_rail_variable_set_and_never_read_is_found() {
    let mut pairs = COMPLETE.to_vec();
    pairs.extend([
        ("RAIL_PROXY_PROTECTED_HOST", "mcp.example.com"),
        ("RAIL_BLANK", " "),
        ("HOME", "/root"),
    ]);

    let (_, startup) = read_startup(&pairs).unwrap();

    assert_eq!(startup.unread, ["RAIL_PROXY_PROTECTED_HOST"]);
}

#[test]
fn nothing_set_and_unused_is_unread() {
    let disabled: Vec<_> = COMPLETE
        .into_iter()
        .map(|(name, value)| match name {
            "RAIL_DISABLED" => (name, "true"),
            _ => (name, value),
        })
        .collect();
    let no_credential: Vec<_> = COMPLETE
        .into_iter()
        .filter_map(|(name, value)| match name {
            "RAIL_AUTH_MODE" => Some((name, "none")),
            "RAIL_AUTH_TOKEN" => None,
            _ => Some((name, value)),
        })
        .collect();
    for pairs in [&COMPLETE[..], &disabled, &no_credential] {
        let (_, startup) = read_startup(pairs).unwrap();
        assert!(startup.unread.is_empty(), "{pairs:?}: {:?}", startup.unread);
    }
}

#[test]
fn rail_center_and_the_ticket_are_read_together() {
    let config = read(&ENABLED).unwrap();

    assert_eq!(
        config.listener,
        Listener::Unix(PathBuf::from("/run/rail/ext.sock"))
    );
    let (rail_center, ticket) = config.ticket.unwrap();
    assert_eq!(rail_center.url.to_string(), "https://rail-center.example");
    assert_eq!(ticket.query.host_id, "host-1");
    assert_eq!(ticket.query.sandbox_name, "sandbox-1");
    assert_eq!(ticket.refresh_interval, Duration::from_secs(600));
    let protected = config.protected_hosts.get("mcp.example.com").unwrap();
    assert_eq!((protected.tls, protected.port), (true, Some(8443)));
}

#[test]
fn rail_center_on_requires_a_protected_host() {
    let unprotected: Vec<_> = ENABLED
        .into_iter()
        .filter(|(name, _)| *name != "RAIL_PROXY_PROTECTED_HOSTS")
        .collect();

    let errors = read(&unprotected).unwrap_err();

    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(
        errors[0].starts_with("RAIL_PROXY_PROTECTED_HOSTS is required because"),
        "{errors:#?}"
    );
}

#[test]
fn disabled_still_reads_the_protected_hosts() {
    let config = read(&[
        ("RAIL_DISABLED", "true"),
        ("RAIL_PROXY_PROTECTED_HOSTS", "http://gw.internal"),
    ])
    .unwrap();
    assert!(config.ticket.is_none());
    assert!(!config.protected_hosts.get("gw.internal").unwrap().tls);

    let errors = read(&[
        ("RAIL_DISABLED", "true"),
        ("RAIL_PROXY_PROTECTED_HOSTS", "ftp://gw.internal"),
    ])
    .unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:#?}");
}

#[test]
fn disabled_reads_no_ticket_but_judges_what_is_set() {
    assert!(read(&[("RAIL_DISABLED", "true")]).unwrap().ticket.is_none());
    let mut set = ENABLED.to_vec();
    set.push(("RAIL_DISABLED", "true"));
    assert!(read(&set).unwrap().ticket.is_none());

    let long = "s".repeat(256);
    let errors = read(&[
        ("RAIL_DISABLED", "true"),
        ("RAIL_CENTER_URL", "ftp://rail-center"),
        ("RAIL_SANDBOX_NAME", &long),
    ])
    .unwrap_err();

    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert!(
        errors[0].starts_with("RAIL_CENTER_URL must start with"),
        "{errors:#?}"
    );
    assert!(
        errors[1].starts_with("RAIL_SANDBOX_NAME has 256 characters"),
        "{errors:#?}"
    );
}

#[test]
fn an_unreadable_disabled_flag_requires_nothing_but_judges_the_rest() {
    assert_eq!(
        read(&[("RAIL_DISABLED", "maybe")]).unwrap_err(),
        ["RAIL_DISABLED must be true or false, got: maybe"]
    );
    assert_eq!(
        read(&[("RAIL_DISABLED", "maybe"), ("RAIL_CENTER_URL", "ftp://x")])
            .unwrap_err()
            .len(),
        2
    );
}

#[test]
fn the_socket_path_is_set_absolute() {
    let config = read(&[
        ("RAIL_DISABLED", "true"),
        ("RAIL_EXT_SOCKET", "/tmp/x.sock"),
    ])
    .unwrap();
    assert_eq!(
        config.listener,
        Listener::Unix(PathBuf::from("/tmp/x.sock"))
    );

    let errors = read(&[("RAIL_DISABLED", "true"), ("RAIL_EXT_SOCKET", "x.sock")]).unwrap_err();
    assert_eq!(errors, ["RAIL_EXT_SOCKET must be an absolute path"]);
}

#[test]
fn it_listens_on_exactly_one_of_the_socket_and_the_port() {
    let (config, startup) =
        read_startup(&[("RAIL_DISABLED", "true"), ("RAIL_CALLOUT_PORT", "8080")]).unwrap();
    assert_eq!(config.listener, Listener::Tcp(8080));
    let described: Vec<_> = startup.entries.iter().map(|entry| entry.name).collect();
    assert!(described.contains(&"RAIL_CALLOUT_PORT"), "{described:?}");
    assert!(!described.contains(&"RAIL_EXT_SOCKET"), "{described:?}");

    let both = read(&[
        ("RAIL_DISABLED", "true"),
        ("RAIL_EXT_SOCKET", "/tmp/x.sock"),
        ("RAIL_CALLOUT_PORT", "8080"),
    ])
    .unwrap_err();
    assert_eq!(
        both,
        [
            "RAIL_EXT_SOCKET and RAIL_CALLOUT_PORT are both set: set only one, the socket or the port to listen on"
        ]
    );
    let neither =
        rail_test_support::read(&[("RAIL_DISABLED", "true")], ProxyConfig::read).unwrap_err();
    assert_eq!(
        neither,
        [
            "RAIL_EXT_SOCKET or RAIL_CALLOUT_PORT is required, the socket or the port to listen on, and neither is set"
        ]
    );
    // A port refused is refused for itself, not as a second listener.
    let refused = read(&[
        ("RAIL_DISABLED", "true"),
        ("RAIL_EXT_SOCKET", "/tmp/x.sock"),
        ("RAIL_CALLOUT_PORT", "0"),
    ])
    .unwrap_err();
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(
        refused[0].starts_with("RAIL_CALLOUT_PORT must be a port"),
        "{refused:?}"
    );
}

#[test]
fn every_refusal_is_listed_at_once() {
    let errors = read(&[
        ("RAIL_CENTER_URL", "http://rail-center"),
        ("RAIL_TICKET_REFRESH_SECONDS", "0"),
        ("RAIL_EXT_SOCKET", "x.sock"),
    ])
    .unwrap_err();

    assert_eq!(errors.len(), 6, "{errors:#?}");
}

#[test]
fn the_gateway_reads_its_own_settings_and_refuses_a_cap_past_the_message_limit() {
    let gateway = |pairs: &[(&str, &str)]| {
        rail_test_support::read(&listening(pairs), GatewayConfig::read).map(|(config, _)| config)
    };
    let config = gateway(&[("RAIL_DISABLED", "true")]).unwrap();
    assert_eq!(
        (config.max_message_bytes, config.max_body_bytes),
        (16 * 1024 * 1024, 15 * 1024 * 1024)
    );
    assert!(config.rail_center.is_none());

    let enabled = gateway(&[
        ("RAIL_CENTER_URL", "https://rail-center.example"),
        ("RAIL_GATEWAY_SLUG", "gw-1"),
    ])
    .unwrap();
    assert_eq!(enabled.rail_center.unwrap().1.gateway_slug, "gw-1");

    // Below the limit is fine; at it, refused, naming both.
    assert!(gateway(&[("RAIL_DISABLED", "true"), ("RAIL_MAX_BODY_MB", "15")]).is_ok());
    let errors = gateway(&[("RAIL_DISABLED", "true"), ("RAIL_MAX_BODY_MB", "16")]).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("RAIL_MAX_BODY_MB must be below RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB"),
        "{errors:?}"
    );
}
