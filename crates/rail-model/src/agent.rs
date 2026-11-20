//! Who a host is, as Rail Center names it.

/// The agent a host serves: the host, and the one sandbox it serves. Its tickets and its capture
/// plan are fetched for it, and a plan names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub host_id: String,
    pub sandbox_name: String,
}
