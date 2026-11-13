//! The proxy's readiness: a ticket valid now, its state in `x-rail-status`'s words.

use std::sync::Arc;

use rail_grpc_host::{RoleCheck, check_held};
use rail_runtime::TicketHolder;

/// Ready while `tickets` holds a valid ticket, read on each check; always, with Rail Center
/// disabled.
pub fn check_proxy(tickets: Option<Arc<TicketHolder>>) -> RoleCheck {
    check_held(tickets, |tickets| match tickets.current() {
        Ok(_) => (true, "valid"),
        Err(unavailable) => (false, unavailable.as_str()),
    })
}
