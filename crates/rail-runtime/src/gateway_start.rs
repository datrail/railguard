//! The gateway role started for a host: its holders and reporter on the Rail Center thread.

use std::sync::Arc;
use std::time::SystemTime;

use crate::bundle::{BundleSettings, RailCenterBundles};
use crate::bundle_holder::BundleHolder;
use crate::client::RailCenterClient;
use crate::clock::Clock;
use crate::denial::{DenialReporter, RailCenterDenials};
use crate::gateway_call::{Gateway, GatewaySettings};
use crate::holder::Jitter;
use crate::key_set::{KeySetHolder, RailCenterKeys};
use crate::rail_center::RailCenterSettings;
use crate::rail_center_thread::RailCenterThread;

// The bundle, the key set and the denial reports go through the Rail Center thread; calls only
// read what's held.
pub fn start_gateway(
    (rail_center, bundle): (RailCenterSettings, BundleSettings),
    settings: GatewaySettings,
) -> Result<(RailCenterThread, Arc<Gateway>), String> {
    let client = RailCenterClient::new(&rail_center)?;
    let thread = RailCenterThread::start()?;
    let runtime = thread.runtime();
    let bundles = Arc::new(BundleHolder::start(
        runtime,
        RailCenterBundles {
            client: client.clone(),
            gateway_slug: bundle.gateway_slug,
        },
        bundle.refresh_interval,
        Jitter::random(),
    ));
    let key_sets = Arc::new(KeySetHolder::start(
        runtime,
        RailCenterKeys {
            client: client.clone(),
        },
        Clock::system(),
        Jitter::random(),
    ));
    let reporter = Arc::new(DenialReporter::new(
        runtime,
        RailCenterDenials { client },
        Arc::new(SystemTime::now),
    ));
    let gateway = Gateway::new(
        bundles,
        key_sets,
        reporter,
        Arc::new(SystemTime::now),
        settings,
    );
    Ok((thread, Arc::new(gateway)))
}
