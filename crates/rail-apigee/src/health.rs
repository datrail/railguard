//! The gateway's readiness: a bundle held.

use std::sync::Arc;

use rail_grpc_host::{RoleCheck, RoleState};
use rail_runtime::Gateway;

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
