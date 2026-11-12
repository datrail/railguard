//! The gateway's readiness: a bundle held.

use std::sync::Arc;

use rail_grpc_host::{RoleCheck, RoleState};
use rail_runtime::BundleHolder;

/// Ready once `bundles` holds a bundle, read on each check; always, with Rail Center disabled.
pub fn check_gateway(bundles: Option<Arc<BundleHolder>>) -> RoleCheck {
    match bundles {
        None => Arc::new(|| RoleState::DISABLED),
        Some(bundles) => Arc::new(move || match bundles.snapshot() {
            Some(_) => RoleState {
                ready: true,
                state: "held",
            },
            None => RoleState {
                ready: false,
                state: "none",
            },
        }),
    }
}
