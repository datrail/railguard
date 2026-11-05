use std::path::PathBuf;

use rail_runtime::{
    ProtectedHosts, RailCenterSettings, Refused, Settings, TicketSettings, read_disabled,
    read_protected_hosts, read_rail_center, read_ticket_settings,
};

const EXT_SOCKET: &str = "RAIL_EXT_SOCKET";
const DEFAULT_SOCKET: &str = "/run/rail/ext.sock";

/// What this process runs with.
#[derive(Debug)]
pub struct Config {
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
        let (socket, rail_center, ticket, protected_hosts) =
            (socket?, rail_center?, ticket?, protected_hosts?);
        // Enabled, both are required, so both are read.
        let ticket = if enabled {
            rail_center.zip(ticket)
        } else {
            None
        };
        Ok(Self {
            socket,
            ticket,
            protected_hosts,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use rail_runtime::{Level, Source, Startup};

    use super::Config;
    use crate::test_settings;

    const TOKEN: &str = "rc_svc_abcdefghijklmnopqrstuvwxyzABCDEFGHIJK-_0123";

    // Every setting, set.
    const COMPLETE: [(&str, &str); 10] = [
        ("RAIL_EXT_SOCKET", "/tmp/rail/ext.sock"),
        ("RAIL_DISABLED", "false"),
        ("RAIL_CENTER_URL", "http://rail-center.internal:8080"),
        ("RAIL_ALLOW_HTTP", "true"),
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

    fn read_startup(pairs: &[(&str, &str)]) -> Result<(Config, Startup), Vec<String>> {
        test_settings::read(pairs, Config::read)
    }

    fn read(pairs: &[(&str, &str)]) -> Result<Config, Vec<String>> {
        read_startup(pairs).map(|(config, _)| config)
    }

    // Each entry logged, as (name, value, level); `pairs` must be accepted.
    fn describe(pairs: &[(&str, &str)]) -> Vec<(&'static str, String, Level)> {
        let (_, startup) = read_startup(pairs).unwrap();
        startup
            .entries
            .into_iter()
            .map(|entry| (entry.name, entry.value, entry.level))
            .collect()
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
            ("RAIL_ALLOW_HTTP", "true"),
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
                ("RAIL_CENTER_URL", "unset".to_owned(), Level::Info),
                ("RAIL_ALLOW_HTTP", "true".to_owned(), Level::Info),
                ("RAIL_AUTH_MODE", "bearer".to_owned(), Level::Info),
                ("RAIL_AUTH_TOKEN", "set".to_owned(), Level::Info),
                ("RAIL_HOST_ID", "unset".to_owned(), Level::Info),
                ("RAIL_SANDBOX_NAME", "unset".to_owned(), Level::Info),
                ("RAIL_TICKET_REFRESH_SECONDS", "600".to_owned(), Level::Info),
                ("RAIL_PROXY_PROTECTED_HOSTS", "none".to_owned(), Level::Info),
            ]
        );
        assert_no_token(&entries);
    }

    #[test]
    fn the_token_is_in_no_entry_whichever_way_it_is_read() {
        let disabled: Vec<_> = COMPLETE
            .into_iter()
            .map(|(name, value)| match name {
                "RAIL_DISABLED" => (name, "true"),
                _ => (name, value),
            })
            .collect();
        for pairs in [&COMPLETE[..], &disabled] {
            assert_no_token(&describe(pairs));
        }
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
            entries[4..6],
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
                ("RAIL_EXT_SOCKET", "/tmp/rail/ext.sock", Level::Info),
                ("RAIL_DISABLED", "false", Level::Info),
                (
                    "RAIL_CENTER_URL",
                    "http://rail-center.internal:8080",
                    Level::Warn("the credential and what Rail Center returns cross in the clear")
                ),
                ("RAIL_ALLOW_HTTP", "true", Level::Info),
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
                (
                    "RAIL_EXT_SOCKET",
                    "/run/rail/ext.sock",
                    Level::Info,
                    Source::Default
                ),
                (
                    "RAIL_DISABLED",
                    "true",
                    Level::Warn("Rail Center isn't used: nothing is fetched from it"),
                    Source::Env
                ),
                ("RAIL_CENTER_URL", "unset", Level::Info, Source::Default),
                ("RAIL_ALLOW_HTTP", "false", Level::Info, Source::Default),
                ("RAIL_AUTH_MODE", "none", Level::Info, Source::Default),
                ("RAIL_AUTH_TOKEN", "unset", Level::Info, Source::Default),
                ("RAIL_HOST_ID", "unset", Level::Info, Source::Default),
                ("RAIL_SANDBOX_NAME", "unset", Level::Info, Source::Default),
                (
                    "RAIL_TICKET_REFRESH_SECONDS",
                    "3600",
                    Level::Info,
                    Source::Default
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

        assert_eq!(config.socket, Path::new("/run/rail/ext.sock"));
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
        assert_eq!(config.socket, Path::new("/tmp/x.sock"));

        let errors = read(&[("RAIL_DISABLED", "true"), ("RAIL_EXT_SOCKET", "x.sock")]).unwrap_err();
        assert_eq!(errors, ["RAIL_EXT_SOCKET must be an absolute path"]);
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
}
