//! Who a host is, as Rail Center names it.

/// Whose ticket to fetch: this host, and the one sandbox it serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TicketQuery {
    pub host_id: String,
    pub sandbox_name: String,
}
