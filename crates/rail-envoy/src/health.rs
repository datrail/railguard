//! The proxy's readiness: a ticket valid now, its state in `x-rail-status`'s words.

use std::sync::Arc;

use rail_grpc_host::{RoleCheck, RoleState};
use rail_runtime::TicketHolder;

/// Ready while `tickets` holds a valid ticket, read on each check; always, with Rail Center
/// disabled.
pub fn check_proxy(tickets: Option<Arc<TicketHolder>>) -> RoleCheck {
    match tickets {
        None => Arc::new(|| RoleState::DISABLED),
        Some(tickets) => Arc::new(move || match tickets.current() {
            Ok(_) => RoleState {
                ready: true,
                state: "valid",
            },
            Err(unavailable) => RoleState {
                ready: false,
                state: unavailable.as_str(),
            },
        }),
    }
}
