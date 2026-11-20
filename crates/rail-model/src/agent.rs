//! Which agent a host serves, as Rail Center names it.

/// Names the agent a host serves: the host, and the one sandbox it serves. Its tickets are
/// fetched for it, and a capture plan names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentId {
    pub host_id: String,
    pub sandbox_name: String,
}
