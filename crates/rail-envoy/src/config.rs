use std::path::PathBuf;

use rail_runtime::{
    Described, ProtectedHosts, RailCenterSettings, Settings, TicketSettings, describe_disabled,
    read_disabled, read_protected_hosts, read_rail_center, read_ticket_settings,
};

pub const EXT_SOCKET: &str = "RAIL_EXT_SOCKET";
const DEFAULT_SOCKET: &str = "/run/rail/ext.sock";

/// What this process runs with.
#[derive(Debug)]
pub struct Config {
    pub socket: PathBuf,
    /// Where this proxy's ticket comes from; `None` when Rail Center is disabled.
    pub ticket: Option<(RailCenterSettings, TicketSettings)>,
    /// The hosts the ticket goes to, and how; routed even when Rail Center is disabled.
    pub protected_hosts: ProtectedHosts,
    /// Every setting read: as used, or as read if it's set and nothing used it.
    pub described: Described,
}

impl Config {
    /// The configuration, `None` if anything is refused; `settings` keeps the refusals.
    pub fn read(settings: &mut Settings) -> Option<Self> {
        let before = settings.refusals();
        let socket = settings.get(EXT_SOCKET).parsed(|text| {
            let path = PathBuf::from(text);
            path.is_absolute()
                .then_some(path)
                .ok_or_else(|| "must be an absolute path".to_owned())
        });
        // Whatever is set is judged either way; only an enabled Rail Center requires its settings.
        let enabled = read_disabled(settings) == Some(false);
        let rail_center = read_rail_center(settings, enabled);
        let ticket = read_ticket_settings(settings, enabled);
        let protected_hosts = read_protected_hosts(settings, enabled);
        if settings.refusals() > before {
            return None;
        }
        let socket = socket.unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET));
        let protected_hosts = protected_hosts
            .expect("read_protected_hosts returns the hosts when it refuses nothing");
        let mut described = Described::default();
        described.info(EXT_SOCKET, socket.display());
        describe_disabled(!enabled, &mut described);
        if let Some(rail_center) = &rail_center {
            rail_center.describe(&mut described);
        }
        if let Some(ticket) = &ticket {
            ticket.describe(&mut described);
        }
        protected_hosts.describe(&mut described);
        settings.describe_rest(&mut described);
        Some(Self {
            socket,
            ticket: enabled.then(|| {
                rail_center
                    .zip(ticket)
                    .expect("the readers return what they require when nothing is refused")
            }),
            protected_hosts,
            described,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::time::Duration;

    use rail_runtime::{Level, Settings, Variables};

    use super::Config;

    const TOKEN: &str = "rc_svc_abcdefghijklmnopqrstuvwxyzABCDEFGHIJK-_0123";

    // Every setting, set; one added to the reader must be added here.
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

    // COMPLETE with each change made: a name set to another value, or unset by `None`.
    fn complete_but(changes: &[(&str, Option<&'static str>)]) -> Vec<(&'static str, &'static str)> {
        COMPLETE
            .into_iter()
            .filter_map(|(name, value)| {
                match changes.iter().find(|(changed, _)| *changed == name) {
                    Some((_, changed)) => changed.map(|changed| (name, changed)),
                    None => Some((name, value)),
                }
            })
            .collect()
    }

    const ENABLED: [(&str, &str); 5] = [
        ("RAIL_CENTER_URL", "https://rail-center.example"),
        ("RAIL_PROXY_PROTECTED_HOSTS", "mcp.example.com:8443"),
        ("RAIL_HOST_ID", "host-1"),
        ("RAIL_SANDBOX_NAME", "sandbox-1"),
        ("RAIL_TICKET_REFRESH_SECONDS", "600"),
    ];

    fn read(pairs: &[(&str, &str)]) -> Result<Config, Vec<String>> {
        let variables: Variables = pairs.iter().copied().collect();
        let mut settings = Settings::new(&variables);
        let config = Config::read(&mut settings);
        match settings.finish() {
            Ok(()) => Ok(config.expect("Config::read returns the config when it refuses nothing")),
            Err(errors) => {
                assert!(config.is_none(), "nothing is read when anything is refused");
                Err(errors.0)
            }
        }
    }

    #[test]
    fn every_setting_read_is_described() {
        // Each branch, with the names it reads though they're unset.
        let fixtures = [
            ("bearer", complete_but(&[]), &[][..]),
            (
                "disabled",
                complete_but(&[("RAIL_DISABLED", Some("true"))]),
                &[],
            ),
            (
                "no credential",
                complete_but(&[("RAIL_AUTH_MODE", Some("none")), ("RAIL_AUTH_TOKEN", None)]),
                &["RAIL_AUTH_TOKEN"],
            ),
        ];
        let secret = TOKEN.strip_prefix("rc_svc_").unwrap();
        for (fixture, pairs, unset) in fixtures {
            let variables: Variables = pairs.iter().copied().collect();
            let mut settings = Settings::new(&variables);

            let config = Config::read(&mut settings).unwrap();

            let read: BTreeSet<&str> = settings.names_read().iter().copied().collect();
            let expected: BTreeSet<&str> = pairs
                .iter()
                .map(|(name, _)| *name)
                .chain(unset.iter().copied())
                .collect();
            assert_eq!(read, expected, "{fixture}");
            assert_eq!(config.described.names(), read, "{fixture}");
            assert_eq!(
                settings.find_unread("RAIL_"),
                Vec::<String>::new(),
                "{fixture}"
            );
            for entry in config.described.entries() {
                assert!(
                    !entry.value.contains(secret),
                    "{fixture}: {} shows the token",
                    entry.name
                );
            }
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

        let config = read(&pairs).unwrap();

        let names = config.described.names();
        for (name, _) in pairs {
            assert!(names.contains(name), "{name} has no entry");
        }
        let entries: Vec<_> = config
            .described
            .entries()
            .iter()
            .map(|entry| (entry.name, entry.value.as_str(), entry.level))
            .collect();
        assert_eq!(
            entries,
            [
                ("RAIL_EXT_SOCKET", "/run/rail/ext.sock", Level::Info),
                (
                    "RAIL_DISABLED",
                    "true",
                    Level::Warn("Rail Center isn't used: nothing is fetched from it")
                ),
                ("RAIL_PROXY_PROTECTED_HOSTS", "none", Level::Info),
                ("RAIL_ALLOW_HTTP", "true", Level::Info),
                ("RAIL_AUTH_MODE", "bearer", Level::Info),
                ("RAIL_AUTH_TOKEN", "set", Level::Info),
                ("RAIL_TICKET_REFRESH_SECONDS", "600", Level::Info),
            ]
        );
        let secret = TOKEN.strip_prefix("rc_svc_").unwrap();
        for (name, value, _) in entries {
            assert!(!value.contains(secret), "{name} shows the token");
        }
    }

    #[test]
    fn the_description_says_each_value_as_used_and_never_the_token() {
        let variables: Variables = COMPLETE.into_iter().collect();
        let config = Config::read(&mut Settings::new(&variables)).unwrap();

        let entries: Vec<_> = config
            .described
            .entries()
            .iter()
            .map(|entry| (entry.name, entry.value.as_str(), entry.level))
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
        let secret = TOKEN.strip_prefix("rc_svc_").unwrap();
        for (name, value, _) in entries {
            assert!(!value.contains(secret), "{name} shows the token");
        }
    }

    #[test]
    fn the_defaults_are_described_and_disabled_as_a_warning() {
        let config = read(&[("RAIL_DISABLED", "true")]).unwrap();

        let entries: Vec<_> = config
            .described
            .entries()
            .iter()
            .map(|entry| (entry.name, entry.value.as_str(), entry.level))
            .collect();
        assert_eq!(
            entries,
            [
                ("RAIL_EXT_SOCKET", "/run/rail/ext.sock", Level::Info),
                (
                    "RAIL_DISABLED",
                    "true",
                    Level::Warn("Rail Center isn't used: nothing is fetched from it")
                ),
                ("RAIL_PROXY_PROTECTED_HOSTS", "none", Level::Info),
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
        let variables: Variables = pairs.into_iter().collect();
        let mut settings = Settings::new(&variables);

        assert!(Config::read(&mut settings).is_some());

        assert_eq!(settings.find_unread("RAIL_"), ["RAIL_PROXY_PROTECTED_HOST"]);
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
