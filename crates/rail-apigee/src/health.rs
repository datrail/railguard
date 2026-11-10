//! `grpc.health.v1`: `""` is liveness, and `datrail.gateway.Ready` readiness, read from the bundle
//! holder on each check of it.

use std::sync::Arc;

use rail_runtime::BundleHolder;
use tokio::sync::Mutex;
use tonic_health::ServingStatus;
use tonic_health::pb::health_server::Health;
use tonic_health::pb::{HealthCheckRequest, HealthCheckResponse};
use tonic_health::server::{HealthReporter, HealthService};

/// The service a readiness check names.
pub const READY: &str = "datrail.gateway.Ready";

/// Sets what the health service answers; clones share it.
#[derive(Clone)]
pub struct Statuses {
    reporter: HealthReporter,
    // `None` with Rail Center disabled, which is always ready.
    bundles: Option<Arc<BundleHolder>>,
    // Whether shutdown started; held across each change, so none can follow it.
    stopping: Arc<Mutex<bool>>,
}

impl Statuses {
    /// Liveness SERVING, and readiness as `bundles` holds a bundle.
    pub async fn start(bundles: Option<Arc<BundleHolder>>) -> (Self, Readiness) {
        let reporter = HealthReporter::new();
        let statuses = Self {
            reporter: reporter.clone(),
            bundles,
            stopping: Arc::default(),
        };
        statuses
            .reporter
            .set_service_status("", ServingStatus::Serving)
            .await;
        statuses.update_readiness().await;
        let service = Readiness {
            inner: HealthService::from_health_reporter(reporter),
            statuses: statuses.clone(),
        };
        (statuses, service)
    }

    /// Both NOT_SERVING, for good.
    pub async fn stop(&self) {
        let mut stopping = self.stopping.lock().await;
        *stopping = true;
        for service in ["", READY] {
            self.reporter
                .set_service_status(service, ServingStatus::NotServing)
                .await;
        }
    }

    // `set_service_status` also tells anyone watching.
    async fn update_readiness(&self) {
        let stopping = self.stopping.lock().await;
        if *stopping {
            return;
        }
        let ready = self
            .bundles
            .as_ref()
            .is_none_or(|bundles| bundles.snapshot().is_some());
        let status = if ready {
            ServingStatus::Serving
        } else {
            ServingStatus::NotServing
        };
        self.reporter.set_service_status(READY, status).await;
    }
}

/// tonic-health's service, with readiness brought up to date before each check of it.
pub struct Readiness {
    inner: HealthService,
    statuses: Statuses,
}

#[tonic::async_trait]
impl Health for Readiness {
    async fn check(
        &self,
        request: tonic::Request<HealthCheckRequest>,
    ) -> Result<tonic::Response<HealthCheckResponse>, tonic::Status> {
        if request.get_ref().service == READY {
            self.statuses.update_readiness().await;
        }
        self.inner.check(request).await
    }

    type WatchStream = <HealthService as Health>::WatchStream;

    async fn watch(
        &self,
        request: tonic::Request<HealthCheckRequest>,
    ) -> Result<tonic::Response<Self::WatchStream>, tonic::Status> {
        self.inner.watch(request).await
    }
}
