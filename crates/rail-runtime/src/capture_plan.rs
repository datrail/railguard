use std::future::Future;
use std::time::Duration;

use rail_model::agent::AgentId;

use crate::client::{FetchError, Limits, RailCenterClient, Response};
use crate::config::{Refused, Settings};
use crate::holder::parse_refresh_interval;
use crate::ticket::agent_query;

const ROUTE: &str = "/v1/capture-plan";

// As a bundle's: a plan is a few dozen entries, so past 4 MiB is a fault, not a large agent.
const LIMITS: Limits = Limits {
    deadline: Duration::from_secs(10),
    silence: Duration::from_secs(5),
    max_bytes: 4 * 1024 * 1024,
};

const CAPTURE_PLAN_REFRESH_SECONDS: &str = "RAIL_CAPTURE_PLAN_REFRESH_SECONDS";

// Every few minutes: a plan has no expiry, and a Rail Center without the route costs one quiet GET.
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

/// How often the capture plan is fetched again, before jitter; described as read.
pub fn read_capture_plan_refresh(settings: &mut Settings) -> Result<Duration, Refused> {
    let refresh_interval = settings
        .get(CAPTURE_PLAN_REFRESH_SECONDS)
        .parsed(parse_refresh_interval)?
        .unwrap_or(DEFAULT_REFRESH_INTERVAL);
    settings.describe(CAPTURE_PLAN_REFRESH_SECONDS, refresh_interval.as_secs());
    Ok(refresh_interval)
}

/// Where capture plans come from: Rail Center, or a test's script.
pub trait CapturePlanSource: Send + Sync + 'static {
    /// One fetch, whatever its status: what each means is the holder's.
    fn fetch(&self) -> impl Future<Output = Result<Response, FetchError>> + Send;

    /// The agent whose plan is fetched, which a plan must name.
    fn agent(&self) -> &AgentId;
}

/// This agent's capture plan, fetched from Rail Center.
pub struct RailCenterCapturePlans {
    pub client: RailCenterClient,
    pub agent: AgentId,
}

impl CapturePlanSource for RailCenterCapturePlans {
    async fn fetch(&self) -> Result<Response, FetchError> {
        self.client
            .get(ROUTE, &agent_query(&self.agent), LIMITS)
            .await
    }

    fn agent(&self) -> &AgentId {
        &self.agent
    }
}
