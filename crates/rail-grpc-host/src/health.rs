//! `grpc.health.v1`: `""`, and each service a host names, is liveness; `rail.Ready` is readiness,
//! SERVING while every role the process runs is ready, read from each role on each check of it,
//! with each role's state in the answer's metadata, `rail-ready-<role>: <state>`.

use std::sync::Arc;

use tokio::sync::Mutex;
use tonic::metadata::{MetadataKey, MetadataValue};
use tonic_health::ServingStatus;
use tonic_health::pb::health_server::Health;
use tonic_health::pb::{HealthCheckRequest, HealthCheckResponse};
use tonic_health::server::{HealthReporter, HealthService};

use crate::Role;

/// The service a readiness check names, whatever the role.
pub const READY: &str = "rail.Ready";

/// Whether a role is ready, and the state it's in, in the role's own words: what a breakdown per
/// role would report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoleState {
    pub ready: bool,
    /// A metadata value: visible ASCII.
    pub state: &'static str,
}

impl RoleState {
    /// With Rail Center disabled, a role has nothing to wait for.
    pub const DISABLED: Self = Self {
        ready: true,
        state: "disabled",
    };
}

/// How a role tells its state, on each readiness check.
pub type RoleCheck = Arc<dyn Fn() -> RoleState + Send + Sync>;

/// Sets what the health service answers; clones share it.
#[derive(Clone)]
pub struct Statuses {
    reporter: HealthReporter,
    // `""` and the host's own services, SERVING while it serves.
    live: Arc<[&'static str]>,
    roles: Arc<[(Role, RoleCheck)]>,
    // Whether shutdown started; held across each change, so none can follow it.
    stopping: Arc<Mutex<bool>>,
}

impl Statuses {
    /// Liveness SERVING for `""` and each of `services`, and readiness as `roles` say.
    pub async fn start(
        services: &[&'static str],
        roles: Vec<(Role, RoleCheck)>,
    ) -> (Self, Readiness) {
        let reporter = HealthReporter::new();
        let live: Vec<&'static str> = std::iter::once("")
            .chain(services.iter().copied())
            .collect();
        let statuses = Self {
            reporter: reporter.clone(),
            live: live.into(),
            roles: roles.into(),
            stopping: Arc::default(),
        };
        for &service in statuses.live.iter() {
            statuses
                .reporter
                .set_service_status(service, ServingStatus::Serving)
                .await;
        }
        // Before anything can stop it, and before the first check, for a watch opened first.
        statuses.set_readiness(&statuses.read_roles()).await;
        let service = Readiness {
            inner: HealthService::from_health_reporter(reporter),
            statuses: statuses.clone(),
        };
        (statuses, service)
    }

    /// Every service NOT_SERVING, for good.
    pub async fn stop(&self) {
        let mut stopping = self.stopping.lock().await;
        *stopping = true;
        for &service in self.live.iter().chain([&READY]) {
            self.reporter
                .set_service_status(service, ServingStatus::NotServing)
                .await;
        }
    }

    /// Each role's state now, in the order given.
    pub fn read_roles(&self) -> Vec<(Role, RoleState)> {
        self.roles
            .iter()
            .map(|(role, check)| (*role, check()))
            .collect()
    }

    // `set_service_status` also tells anyone watching.
    async fn set_readiness(&self, roles: &[(Role, RoleState)]) {
        let status = if roles.iter().all(|(_, state)| state.ready) {
            ServingStatus::Serving
        } else {
            ServingStatus::NotServing
        };
        self.reporter.set_service_status(READY, status).await;
    }
}

/// The metadata header a readiness answer gives a role's state in, e.g. `rail-ready-proxy`.
pub fn breakdown_header(role: Role) -> String {
    format!("{BREAKDOWN_PREFIX}{role}")
}

pub(crate) const BREAKDOWN_PREFIX: &str = "rail-ready-";

// Once stopping, every role reads so, as the status does.
const STOPPING: &str = "stopping";

/// tonic-health's service, with readiness brought up to date before each check of it, and each
/// role's state in the answer's metadata.
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
        if request.get_ref().service != READY {
            return self.inner.check(request).await;
        }
        // Held until the answer is built, so its status and its breakdown come from one read.
        let stopping = self.statuses.stopping.lock().await;
        let roles = self.statuses.read_roles();
        if !*stopping {
            self.statuses.set_readiness(&roles).await;
        }
        let mut response = self.inner.check(request).await?;
        for (role, state) in roles {
            let state = if *stopping { STOPPING } else { state.state };
            let key = MetadataKey::from_bytes(breakdown_header(role).as_bytes())
                .expect("a role's name is a header name");
            response
                .metadata_mut()
                .insert(key, MetadataValue::from_static(state));
        }
        Ok(response)
    }

    type WatchStream = <HealthService as Health>::WatchStream;

    async fn watch(
        &self,
        request: tonic::Request<HealthCheckRequest>,
    ) -> Result<tonic::Response<Self::WatchStream>, tonic::Status> {
        self.inner.watch(request).await
    }
}
