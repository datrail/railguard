//! The Envoy host, in the proxy role: ext_authz and ext_proc on a unix socket Envoy shares. For
//! now it allows every request, with this proxy's ticket in `x-rail` to a protected host.
//!
//! `rail-envoy` (`rail` in its image) serves, with `grpc.health.v1` on the same socket;
//! `rail-envoy probe` asks it, and exits 0 if the server answers SERVING, for a container health
//! check in an image with no shell.
//!
//! The binary as a library: `run`, and the configuration and callouts it serves, which its tests
//! drive.

mod config;
mod services;
mod xrail;

use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::SystemTime;

use envoy_types::ext_authz::v3::pb::AuthorizationServer;
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessorServer;
use rail_grpc_host::Address;
use rail_runtime::{
    Jitter, RailCenterClient, RailCenterSettings, RailCenterThread, RailCenterTickets, Settings,
    Startup, TicketHolder, TicketSettings, Variables,
};
use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::server::NamedService;
use tonic_health::ServingStatus;
use tonic_health::server::HealthReporter;

pub use crate::config::Config;
pub use crate::services::Callouts;

/// The binary: serves, or with `probe`, asks the server on the socket configured.
pub async fn run() -> ExitCode {
    rail_runtime::logging::init();
    // Before the settings, so a refused configuration names the build; not for a probe, run often.
    if std::env::args().nth(1).is_none() {
        rail_runtime::logging::log_build();
    }
    // The probe reads the same configuration, so it checks the socket where it's served.
    let variables = Variables::from_process();
    let mut settings = Settings::new(&variables);
    let config = Config::read(&mut settings);
    let (config, startup) = settings
        .finish("RAIL_", config)
        .unwrap_or_else(|errors| errors.exit());
    rail_runtime::logging::set_level(config.log_level);
    match std::env::args().nth(1).as_deref() {
        None => serve(config, &startup).await,
        Some("probe") => rail_grpc_host::probe(&Address::Unix(config.socket.clone()), "").await,
        Some(argument) => {
            let program = rail_grpc_host::program_name();
            eprintln!("unknown argument {argument:?}; usage: {program} [probe]");
            // A usage error shares the exit status of a refused configuration.
            ExitCode::from(rail_runtime::EXIT_CODE)
        }
    }
}

async fn serve(config: Config, startup: &Startup) -> ExitCode {
    rail_runtime::logging::log_settings(startup);
    // SIGTERM is how a container is stopped; as PID 1 nothing handles it unless we do.
    let terminate = match signal(SignalKind::terminate()) {
        Ok(terminate) => terminate,
        Err(error) => {
            tracing::error!(error = %error, "can't handle SIGTERM");
            return ExitCode::FAILURE;
        }
    };
    let listener = match bind(&config.socket) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(error = %error, "can't listen on the socket");
            // The socket path is configuration, so it fails as a refused configuration does.
            return ExitCode::from(rail_runtime::EXIT_CODE);
        }
    };
    let (rail_center, tickets) = match config.ticket.map(start_tickets).transpose() {
        Ok(started) => started.unzip(),
        Err(error) => {
            tracing::error!(error = %error, "can't start fetching the ticket");
            return ExitCode::FAILURE;
        }
    };
    let callouts = Callouts {
        tickets,
        protected_hosts: Arc::new(config.protected_hosts),
    };
    // Only answered once serving; the ticket never changes it, so nothing restarts us for it.
    let (health, health_service) = tonic_health::server::health_reporter();
    report_status(&health, ServingStatus::Serving).await;
    tracing::info!("serving Envoy");
    let (draining, drain_started) = oneshot::channel();
    let stopping = async {
        rail_grpc_host::terminated(terminate).await;
        report_status(&health, ServingStatus::NotServing).await;
        let _ = draining.send(());
    };
    let server = tonic::transport::Server::builder()
        .add_service(health_service)
        .add_service(AuthorizationServer::new(callouts.clone()))
        .add_service(ExternalProcessorServer::new(callouts))
        .serve_with_incoming_shutdown(UnixListenerStream::new(listener), stopping);
    let served = rail_grpc_host::drain_until_deadline(server, drain_started).await;
    // Its stop blocks on a join, so it's kept off the async workers.
    if let Some(thread) = rail_center {
        let _ = tokio::task::spawn_blocking(move || thread.stop()).await;
    }
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = %error, "the gRPC server failed");
            ExitCode::FAILURE
        }
    }
}

// The ticket is fetched and refreshed on the Rail Center thread; calls only read it.
fn start_tickets(
    (rail_center, ticket): (RailCenterSettings, TicketSettings),
) -> Result<(RailCenterThread, Arc<TicketHolder>), String> {
    let client = RailCenterClient::new(&rail_center)?;
    let thread = RailCenterThread::start()?;
    let source = RailCenterTickets {
        client,
        settings: ticket,
    };
    let tickets = TicketHolder::start(
        thread.runtime(),
        source,
        Arc::new(SystemTime::now),
        Jitter::random(),
    );
    Ok((thread, Arc::new(tickets)))
}

// A socket left by an earlier run is replaced; anything else at the path is refused.
fn bind(socket: &Path) -> Result<UnixListener, String> {
    match fs::symlink_metadata(socket) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(socket).map_err(|error| {
                format!(
                    "can't remove the stale socket {}: {error}",
                    socket.display()
                )
            })?;
        }
        Ok(_) => {
            return Err(format!("{} exists and isn't a socket", socket.display()));
        }
        Err(_) => {}
    }
    let listener = UnixListener::bind(socket)
        .map_err(|error| format!("can't create the socket {}: {error}", socket.display()))?;
    // Envoy runs in this socket's group, as another user.
    fs::set_permissions(socket, fs::Permissions::from_mode(0o660)).map_err(|error| {
        format!(
            "can't make the socket {} group-writable: {error}",
            socket.display()
        )
    })?;
    Ok(listener)
}

// The server as a whole, and each callout by its service name.
async fn report_status(health: &HealthReporter, status: ServingStatus) {
    health.set_service_status("", status).await;
    health
        .set_service_status(AuthorizationServer::<Callouts>::NAME, status)
        .await;
    health
        .set_service_status(ExternalProcessorServer::<Callouts>::NAME, status)
        .await;
}
