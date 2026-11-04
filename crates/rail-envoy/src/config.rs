use std::path::PathBuf;

use rail_runtime::{
    ConfigErrors, DISABLED, RailCenter, RailCenterSettings, Settings, TICKET_SETTINGS,
    TicketSettings, Variables, read_rail_center, read_ticket_settings,
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
        let ticket = match read_rail_center(&mut settings) {
            Some(RailCenter::Enabled(rail_center)) => {
                read_ticket_settings(&mut settings).map(|ticket| Some((rail_center, ticket)))
            }
            Some(RailCenter::Disabled) => {
                refuse_beside_disabled(&mut settings);
                Some(None)
            }
            // Refused: the ticket's settings are judged anyway, so every refusal is listed at once.
            None => {
                match settings
                    .text(DISABLED)
                    .map(|text| text.eq_ignore_ascii_case("true"))
                {
                    Some(true) => refuse_beside_disabled(&mut settings),
                    // Unreadable: whether Rail Center is meant is unknown.
                    None if settings.is_set(DISABLED) => {}
                    _ => {
                        read_ticket_settings(&mut settings);
                    }
                }
                None
            }
        };
        settings.finish()?;
        Ok(Self {
            socket: socket.unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET)),
            ticket: ticket.expect("read when nothing is refused"),
        })
    }
}

fn refuse_beside_disabled(settings: &mut Settings) {
    let set: Vec<&str> = TICKET_SETTINGS
        .into_iter()
        .filter(|name| settings.is_set(name))
        .collect();
    for name in set {
        settings.refuse(format!(
            "{DISABLED} is true, but {name} is set, which only Rail Center reads; unset {name}, \
             or {DISABLED}"
        ));
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
    fn disabled_reads_no_ticket_and_refuses_its_settings() {
        assert!(read(&[("RAIL_DISABLED", "true")]).unwrap().ticket.is_none());

        let errors = read(&[("RAIL_DISABLED", "true"), ("RAIL_SANDBOX_NAME", "s")]).unwrap_err();

        assert_eq!(
            errors,
            [
                "RAIL_DISABLED is true, but RAIL_SANDBOX_NAME is set, which only Rail Center reads; \
              unset RAIL_SANDBOX_NAME, or RAIL_DISABLED"
            ]
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
    fn a_ticket_setting_beside_disabled_is_refused_with_the_rest() {
        let errors = read(&[
            ("RAIL_DISABLED", "true"),
            ("RAIL_CENTER_URL", "https://rail-center.example"),
            ("RAIL_HOST_ID", "h"),
        ])
        .unwrap_err();

        assert_eq!(errors.len(), 2, "{errors:#?}");
        assert!(errors[1].contains("RAIL_HOST_ID is set"), "{errors:#?}");
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
