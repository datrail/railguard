//! Which agent a host serves, as Rail Center names it.

use serde::Deserialize;

/// Names the agent a host serves: the host, and the one sandbox it serves. Its tickets and its
/// capture plan are fetched for it, and a plan names it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentId {
    pub host_id: String,
    pub sandbox_name: String,
}
