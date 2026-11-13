//! The Envoy host: ext_authz and ext_proc on a unix socket Envoy shares, or on a port. As the
//! proxy, it allows every request, with this proxy's ticket in `x-rail` to a protected host; as
//! the gateway, it judges each request against the policy bundle held.
//!
//! `rail-envoy proxy` or `rail-envoy gateway` (`rail proxy`, `rail gateway` in its image)
//! serves, with `grpc.health.v1` on the same listener; `rail-envoy probe [--ready]` asks it for
//! liveness or readiness, and exits 0 if it answers SERVING, for a container health check in an
//! image with no shell.
//!
//! The binary as a library: `run`, and the configuration and callouts it serves, which its tests
//! drive.

mod config;
mod gateway;
mod health;
mod services;
mod xrail;

use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::SystemTime;

use envoy_types::ext_authz::v3::pb::AuthorizationServer;
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessorServer;
use rail_grpc_host::{Address, Command, Role, Statuses, check_held};
use rail_runtime::{
    Gateway, GatewaySettings, Jitter, RailCenterClient, RailCenterSettings, RailCenterThread,
    RailCenterTickets, Settings, Startup, TicketHolder, TicketSettings, Variables, start_gateway,
};
use tokio::net::{TcpListener, UnixListener};
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio_stream::wrappers::{TcpListenerStream, UnixListenerStream};
use tonic::server::NamedService;
use tonic::transport::server::Router;
use tonic_health::pb::health_server::HealthServer;

use crate::config::read_listener;
pub use crate::config::{GatewayConfig, Listener, ProxyConfig};
use crate::gateway::GatewayCallouts;
pub use crate::health::check_proxy;
pub use crate::services::Callouts;

/// The binary: serves as the role named, or with `probe [--ready]`, asks the server where it's
/// configured to listen.
pub async fn run() -> ExitCode {
    rail_runtime::logging::init();
    let command_line: Vec<String> = std::env::args().collect();
    let command = match rail_grpc_host::parse_command(&[Role::Proxy, Role::Gateway], &command_line)
    {
        Ok(command) => command,
        Err(usage) => {
            rail_runtime::logging::log_usage(&usage);
            // A usage error shares the exit status of a refused configuration.
            return ExitCode::from(rail_runtime::EXIT_CODE);
        }
    };
    let variables = Variables::from_process();
    let settings = Settings::new(&variables);
    match command {
        Command::Serve(role) => {
            // Before the settings, so a refused configuration names the build.
            rail_runtime::logging::log_build(role.as_str());
            match role {
                Role::Proxy => {
                    let (config, startup) = settings.finish_or_exit("RAIL_", ProxyConfig::read);
                    rail_runtime::logging::set_level(config.log_level);
                    serve_proxy(config, &startup).await
                }
                Role::Gateway => {
                    let (config, startup) = settings.finish_or_exit("RAIL_", GatewayConfig::read);
                    rail_runtime::logging::set_level(config.log_level);
                    serve_gateway(config, &startup).await
                }
            }
        }
        // Only the listener: the rest is the server's to judge, and the probe names no role.
        Command::Probe { ready } => {
            let (listener, _) = settings.finish_or_exit("RAIL_", read_listener);
            let address = match listener {
                Listener::Unix(socket) => Address::Unix(socket),
                Listener::Tcp(port) => Address::Tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, port))),
            };
            rail_grpc_host::probe(&address, ready).await
        }
    }
}

async fn serve_proxy(config: ProxyConfig, startup: &Startup) -> ExitCode {
    let (terminate, bound) = match prepare(&config.listener, startup).await {
        Ok(prepared) => prepared,
        Err(code) => return code,
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
        vec![(Role::Proxy, check_proxy(callouts.tickets.clone()))],
    )
    .await;
    tracing::info!(listener = %config.listener, "serving Envoy");
    let router = tonic::transport::Server::builder()
        .add_service(HealthServer::new(readiness))
        .add_service(AuthorizationServer::new(callouts.clone()))
        .add_service(ExternalProcessorServer::new(callouts));
    serve_until_stopped(bound, router, statuses, terminate, rail_center).await
}

async fn serve_gateway(config: GatewayConfig, startup: &Startup) -> ExitCode {
    let (terminate, bound) = match prepare(&config.listener, startup).await {
        Ok(prepared) => prepared,
        Err(code) => return code,
    };
    let settings = GatewaySettings {
        max_body_bytes: config.max_body_bytes,
        ..GatewaySettings::default()
    };
    let started = config
        .rail_center
        .map(|rail_center| start_gateway(rail_center, settings))
        .transpose();
    let (rail_center, gateway) = match started {
        Ok(started) => started.unzip(),
        Err(error) => {
            tracing::error!(error = %error, "can't start fetching from Rail Center");
            return ExitCode::FAILURE;
        }
    };
    let callouts = GatewayCallouts {
        gateway: gateway.clone(),
    };
    let (statuses, readiness) = Statuses::start(
        &[
            AuthorizationServer::<GatewayCallouts>::NAME,
            ExternalProcessorServer::<GatewayCallouts>::NAME,
        ],
        vec![(Role::Gateway, check_held(gateway, Gateway::readiness))],
    )
    .await;
    tracing::info!(listener = %config.listener, "serving Envoy");
    let limit = config.max_message_bytes;
    let router = tonic::transport::Server::builder()
        .add_service(HealthServer::new(readiness))
        .add_service(AuthorizationServer::new(callouts.clone()).max_decoding_message_size(limit))
        .add_service(ExternalProcessorServer::new(callouts).max_decoding_message_size(limit));
    serve_until_stopped(bound, router, statuses, terminate, rail_center).await
}

// The settings logged, SIGTERM handled and the listener bound, before anything starts.
async fn prepare(listener: &Listener, startup: &Startup) -> Result<(Signal, Bound), ExitCode> {
    rail_runtime::logging::log_settings(startup);
    // SIGTERM is how a container is stopped; as PID 1 nothing handles it unless we do.
    let terminate = signal(SignalKind::terminate()).map_err(|error| {
        tracing::error!(error = %error, "can't handle SIGTERM");
        ExitCode::FAILURE
    })?;
    let bound = bind(listener).await.map_err(|error| {
        tracing::error!(error = %error, "can't listen");
        // Where it listens is configuration, so it fails as a refused configuration does.
        ExitCode::from(rail_runtime::EXIT_CODE)
    })?;
    Ok((terminate, bound))
}

// One server per incoming type.
async fn serve_until_stopped(
    bound: Bound,
    router: Router,
    statuses: Statuses,
    terminate: Signal,
    rail_center: Option<RailCenterThread>,
) -> ExitCode {
    let stop = rail_center.map(|thread| move || thread.stop());
    match bound {
        Bound::Unix(listener) => {
            let incoming = UnixListenerStream::new(listener);
            rail_grpc_host::serve_until_stopped(router, incoming, statuses, terminate, stop).await
        }
        Bound::Tcp(listener) => {
            let incoming = TcpListenerStream::new(listener);
            rail_grpc_host::serve_until_stopped(router, incoming, statuses, terminate, stop).await
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

enum Bound {
    Unix(UnixListener),
    Tcp(TcpListener),
}

// On every interface for a port, as a callout reached over the network; the probe asks loopback.
async fn bind(listener: &Listener) -> Result<Bound, String> {
    match listener {
        Listener::Unix(socket) => bind_socket(socket).map(Bound::Unix),
        Listener::Tcp(port) => TcpListener::bind((Ipv4Addr::UNSPECIFIED, *port))
            .await
            .map(Bound::Tcp)
            .map_err(|error| format!("can't listen on port {port}: {error}")),
    }
}

// A socket left by an earlier run is replaced; anything else at the path is refused.
fn bind_socket(socket: &Path) -> Result<UnixListener, String> {
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
