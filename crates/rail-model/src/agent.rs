//! Which agent a host serves, as Rail Center names it.

use serde::Deserialize;

/// Names the agent a host serves: the host, and the one sandbox it serves. Its tickets are
/// fetched for it, and a capture plan names it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AgentId {
    pub host_id: String,
    pub sandbox_name: String,
    // `agent_key` goes here: a capture plan may carry it, and Rail Center doesn't send it yet.
}
