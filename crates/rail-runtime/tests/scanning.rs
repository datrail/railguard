//! A bundle slow to judge a call carrying `x-a`, and settings that let only a deadline stop it.
// Shared by both test binaries.

use std::num::NonZeroUsize;
use std::time::Duration;

use rail_policy::Fallback;
use rail_runtime::{GatewaySettings, UnitSettings};
use serde_json::{Value, json};

use crate::bundles;

// Over a minute of scans unoptimised, and seconds optimised, unless stopped within one.
pub const SCANS: u64 = 2_000;
pub const SCANNED: usize = 16 << 20;

// At `mode`, each policy scanning all of `x-a` for a `b` it lacks, so a call carrying it is judged
// slowly, then one denying every call without a ticket.
pub fn scanning(mode: &str) -> Value {
    let mut policies = Vec::from_iter((1..=SCANS).map(|priority| {
        json!({
            "id": format!("5c8f1e42-0000-4000-9000-{priority:012x}"),
            "name": "S",
            "priority": priority,
            "condition": {"path": "request.headers.x-a", "op": "matches", "value": "*b*"},
            "action": "block",
            "enabled": true,
        })
    }));
    let mut denying = bundles::denying_unticketed(json!({}));
    denying["policies"][0]["priority"] = json!(SCANS + 1);
    policies.push(denying["policies"][0].take());
    bundles::denying_unticketed(json!({"policies": policies, "enforcement": {"mode": mode}}))
}

// One judging slot, and no step limit, so only the deadline ends a slow judgement.
pub fn scanning_settings(deadline: Duration) -> GatewaySettings {
    GatewaySettings {
        deadline,
        max_concurrent_judgements: NonZeroUsize::MIN,
        // Not what a call unjudged with a bundle held gets.
        no_bundle_fallback: Fallback::Pass,
        unit: UnitSettings {
            step_limit: u64::MAX,
        },
        ..GatewaySettings::default()
    }
}
