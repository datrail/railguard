//! Each role's readiness: the proxy's, a ticket valid now, in `x-rail-status`'s words; the
//! gateway's, a bundle held.

use std::sync::Arc;

use rail_grpc_host::{RoleCheck, RoleState};
use rail_runtime::{Gateway, TicketHolder};

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

/// Ready once `gateway` holds a bundle, read on each check; always, with Rail Center disabled.
pub fn check_gateway(gateway: Option<Arc<Gateway>>) -> RoleCheck {
    match gateway {
        None => Arc::new(|| RoleState::DISABLED),
        Some(gateway) => Arc::new(move || {
            let (ready, state) = gateway.readiness();
            RoleState { ready, state }
        }),
    }
}
