//! The Envoy host, in the proxy role: ext_authz and ext_proc on a unix socket Envoy shares. For
//! now it allows every request, with this proxy's ticket in `x-rail` to a protected host.
//!
//! `rail-envoy proxy` (`rail proxy` in its image) serves, with `grpc.health.v1` on the same
//! socket; `rail-envoy probe` asks it, and exits 0 if the server answers SERVING, for a container
//! health check in an image with no shell.
//!
//! The binary as a library: `run`, and the configuration and callouts it serves, which its tests
//! drive.

mod config;
mod health;
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
use rail_grpc_host::{Address, Command, Role, Statuses};
use rail_runtime::{
    Jitter, RailCenterClient, RailCenterSettings, RailCenterThread, RailCenterTickets, Settings,
    Startup, TicketHolder, TicketSettings, Variables,
};
use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::server::NamedService;
use tonic_health::pb::health_server::HealthServer;

pub use crate::config::{Config, read_socket};
pub use crate::health::check_proxy;
pub use crate::services::Callouts;

/// The binary: serves as the role named, or with `probe [--ready]`, asks the server on the
/// socket configured.
pub async fn run() -> ExitCode {
    rail_runtime::logging::init();
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let command = match rail_grpc_host::parse_command(&[Role::Proxy], &arguments) {
        Ok(command) => command,
        Err(usage) => {
            eprintln!("{usage}");
            // A usage error shares the exit status of a refused configuration.
            return ExitCode::from(rail_runtime::EXIT_CODE);
        }
    };
    let variables = Variables::from_process();
    let mut settings = Settings::new(&variables);
    match command {
        Command::Serve(role) => {
            // Before the settings, so a refused configuration names the build.
            rail_runtime::logging::log_build(role.as_str());
            let config = Config::read(&mut settings);
            let (config, startup) = settings
                .finish("RAIL_", config)
                .unwrap_or_else(|errors| errors.exit());
            rail_runtime::logging::set_level(config.log_level);
            serve(role, config, &startup).await
        }
        // Only the socket: the rest is the server's to judge, and the probe names no role.
        Command::Probe { ready } => {
            let socket = read_socket(&mut settings);
            let (socket, _) = settings
                .finish("RAIL_", socket)
                .unwrap_or_else(|errors| errors.exit());
            rail_grpc_host::probe(&Address::Unix(socket), rail_grpc_host::probe_service(ready))
                .await
        }
    }
}

async fn serve(role: Role, config: Config, startup: &Startup) -> ExitCode {
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
    // Liveness never reads the ticket, so nothing restarts us for it; readiness does.
    let (statuses, readiness) = Statuses::start(
        &[
            AuthorizationServer::<Callouts>::NAME,
            ExternalProcessorServer::<Callouts>::NAME,
        ],
        vec![(role, check_proxy(callouts.tickets.clone()))],
    )
    .await;
    tracing::info!("serving Envoy");
    let (draining, drain_started) = oneshot::channel();
    let stopping = async {
        rail_grpc_host::terminated(terminate).await;
        statuses.stop().await;
        let _ = draining.send(());
    };
    let server = tonic::transport::Server::builder()
        .add_service(HealthServer::new(readiness))
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
