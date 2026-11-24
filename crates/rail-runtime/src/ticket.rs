use std::future::Future;
use std::time::{Duration, SystemTime};

use rail_model::agent::AgentId;

use crate::client::{FetchError, Limits, RailCenterClient, Response};
use crate::config::{Refused, Secret, Settings};
use crate::holder::parse_refresh_interval;

const ROUTE: &str = "/v1/tickets";

pub(crate) const HOST_ID: &str = "RAIL_HOST_ID";
pub(crate) const SANDBOX_NAME: &str = "RAIL_SANDBOX_NAME";
pub(crate) const TICKET_REFRESH_SECONDS: &str = "RAIL_TICKET_REFRESH_SECONDS";

// Rail Center's bounds on the query (`get_host_tickets`), in characters.
const MAX_HOST_ID_CHARS: usize = 64;
const MAX_SANDBOX_NAME_CHARS: usize = 255;

const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(3600);

// A named fetch answers with one ticket at most, so a body past this is a wrong one.
const LIMITS: Limits = Limits {
    deadline: Duration::from_secs(10),
    silence: Duration::from_secs(5),
    max_bytes: 256 * 1024,
};

/// How this host's ticket is fetched and kept.
#[derive(Clone, Debug)]
pub struct TicketSettings {
    pub agent: AgentId,
    /// The longest a valid ticket goes unrefreshed, before jitter; its own lifetime paces
    /// refreshes below it.
    pub refresh_interval: Duration,
}

/// The ticket's settings, every one set judged, `None` if the host or sandbox is missing; both
/// are required if `required`, which is whether Rail Center is used. Described only when read
/// whole.
pub fn read_ticket_settings(
    settings: &mut Settings,
    required: bool,
) -> Result<Option<TicketSettings>, Refused> {
    let host_id = settings
        .get(HOST_ID)
        .required_if(required)
        .max_chars(MAX_HOST_ID_CHARS)
        .text();
    let sandbox_name = settings
        .get(SANDBOX_NAME)
        .required_if(required)
        .max_chars(MAX_SANDBOX_NAME_CHARS)
        .text();
    let refresh = settings
        .get(TICKET_REFRESH_SECONDS)
        .parsed(parse_refresh_interval)
        .map(|refresh| refresh.unwrap_or(DEFAULT_REFRESH_INTERVAL));
    let (host_id, sandbox_name, refresh_interval) = (host_id?, sandbox_name?, refresh?);
    let ticket = host_id
        .zip(sandbox_name)
        .map(|(host_id, sandbox_name)| TicketSettings {
            agent: AgentId {
                host_id: host_id.to_owned(),
                sandbox_name: sandbox_name.to_owned(),
            },
            refresh_interval,
        });
    if let Some(ticket) = &ticket {
        settings.describe(HOST_ID, &ticket.agent.host_id);
        settings.describe(SANDBOX_NAME, &ticket.agent.sandbox_name);
        settings.describe(TICKET_REFRESH_SECONDS, ticket.refresh_interval.as_secs());
    }
    Ok(ticket)
}

/// A ticket to present, opaque, and the instant it stops being valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpaqueTicket {
    pub token: Secret,
    pub expires_at: SystemTime,
}

impl OpaqueTicket {
    pub fn is_valid(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }
}

/// Where tickets come from: Rail Center, or a test's script.
pub trait TicketSource: Send + Sync + 'static {
    /// One fetch, whatever its status: what each means is the holder's.
    fn fetch(&self) -> impl Future<Output = Result<Response, FetchError>> + Send;

    /// The agent whose ticket is fetched, which a ticket must name.
    fn agent(&self) -> &AgentId;

    /// The longest a valid ticket goes unrefreshed, before jitter.
    fn refresh_interval(&self) -> Duration;
}

/// This host's ticket, fetched from Rail Center.
pub struct RailCenterTickets {
    pub client: RailCenterClient,
    pub settings: TicketSettings,
}

impl TicketSource for RailCenterTickets {
    async fn fetch(&self) -> Result<Response, FetchError> {
        let agent = &self.settings.agent;
        let params = [
            ("host_id", agent.host_id.as_str()),
            ("sandbox_name", agent.sandbox_name.as_str()),
        ];
        self.client.get(ROUTE, &params, LIMITS).await
    }

    fn agent(&self) -> &AgentId {
        &self.settings.agent
    }

    fn refresh_interval(&self) -> Duration {
        self.settings.refresh_interval
    }
}
