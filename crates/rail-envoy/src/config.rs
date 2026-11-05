use std::path::PathBuf;

use rail_runtime::{
    ConfigErrors, RailCenterSettings, Settings, TicketSettings, Variables, read_disabled,
    read_rail_center, read_ticket_settings,
};

pub const EXT_SOCKET: &str = "RAIL_EXT_SOCKET";
const DEFAULT_SOCKET: &str = "/run/rail/ext.sock";

/// What this process runs with.
#[derive(Debug)]
pub struct Config {
    pub socket: PathBuf,
    /// Where this proxy's ticket comes from; `None` when Rail Center is disabled.
    pub ticket: Option<(RailCenterSettings, TicketSettings)>,
}

impl Config {
    pub fn read(variables: &Variables) -> Result<Self, ConfigErrors> {
        let mut settings = Settings::new(variables);
        let socket = settings.parse(EXT_SOCKET, |text| {
            let path = PathBuf::from(text);
            path.is_absolute()
                .then_some(path)
                .ok_or_else(|| "must be an absolute path".to_owned())
        });
        // Whatever is set is judged either way; only an enabled Rail Center requires its settings.
        let enabled = read_disabled(&mut settings) == Some(false);
        let rail_center = read_rail_center(&mut settings, enabled);
        let ticket = read_ticket_settings(&mut settings, enabled);
        settings.finish()?;
        Ok(Self {
            socket: socket.unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET)),
            ticket: enabled.then(|| {
                rail_center
                    .zip(ticket)
                    .expect("the readers return what they require when nothing is refused")
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use rail_runtime::Variables;

    use super::Config;

    const ENABLED: [(&str, &str); 4] = [
        ("RAIL_CENTER_URL", "https://rail-center.example"),
        ("RAIL_HOST_ID", "host-1"),
        ("RAIL_SANDBOX_NAME", "sandbox-1"),
        ("RAIL_TICKET_REFRESH_SECONDS", "600"),
    ];

    fn read(pairs: &[(&str, &str)]) -> Result<Config, Vec<String>> {
        let variables: Variables = pairs.iter().copied().collect();
        Config::read(&variables).map_err(|errors| errors.0)
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

        assert_eq!(errors.len(), 5, "{errors:#?}");
    }
}
