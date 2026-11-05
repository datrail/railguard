//! The Envoy host, in the proxy role: ext_authz and ext_proc on a unix socket Envoy shares. For
//! now it allows every request, with this proxy's ticket in `x-rail` to a protected host.
//!
//! `rail-envoy` serves, with `grpc.health.v1` on the same socket; `rail-envoy probe` asks it, and
//! exits 0 if the server answers SERVING, for a container health check in an image with no shell.

mod config;
mod rail_center;
mod services;
mod xrail;

use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::Path;
use std::pin::pin;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use envoy_types::ext_authz::v3::pb::AuthorizationServer;
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessorServer;
use rail_policy::sanitize;
use rail_runtime::{
    Described, Level, RailCenterClient, RailCenterSettings, RailCenterTickets, Settings,
    TicketHolder, TicketSettings, Variables,
};
use tokio::net::UnixListener;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::server::NamedService;
use tonic::transport::Endpoint;
use tonic_health::ServingStatus;
use tonic_health::pb::HealthCheckRequest;
use tonic_health::pb::health_check_response::ServingStatus as WireStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::server::HealthReporter;

use crate::config::Config;
use crate::rail_center::RailCenterThread;
use crate::services::{Allow, Continue};

// Envoy keeps its connections open, and an ext_proc stream lasts as long as its request.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);

// Longer than any answer from a live server, and within the e2e health check's timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> ExitCode {
    rail_runtime::logging::init();
    // The probe reads the same configuration, so it checks the socket where it's served.
    let variables = Variables::from_process();
    let mut settings = Settings::new(&variables);
    let config = Config::read(&mut settings);
    let unread = settings.find_unread("RAIL_");
    settings.finish().unwrap_or_else(|errors| errors.exit());
    let config = config.expect("Config::read returns the config when it refuses nothing");
    match std::env::args().nth(1).as_deref() {
        None => serve(config, &variables, &unread).await,
        Some("probe") => probe(&config.socket).await,
        Some(argument) => {
            eprintln!("unknown argument {argument:?}; usage: rail-envoy [probe]");
            ExitCode::from(2)
        }
    }
}

async fn serve(config: Config, variables: &Variables, unread: &[String]) -> ExitCode {
    log_settings(&config.described, variables, unread);
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
            return ExitCode::from(2);
        }
    };
    let (rail_center, tickets) = match config.ticket.map(start_tickets).transpose() {
        Ok(started) => started.unzip(),
        Err(error) => {
            tracing::error!(error = %error, "can't start fetching the ticket");
            return ExitCode::FAILURE;
        }
    };
    let protected_hosts = Arc::new(config.protected_hosts);
    // Only answered once serving; the ticket never changes it, so nothing restarts us for it.
    let (health, health_service) = tonic_health::server::health_reporter();
    report_status(&health, ServingStatus::Serving).await;
    tracing::info!("serving Envoy");
    let (draining, drain_started) = oneshot::channel();
    let stopping = async {
        terminated(terminate).await;
        report_status(&health, ServingStatus::NotServing).await;
        let _ = draining.send(());
    };
    let server = tonic::transport::Server::builder()
        .add_service(health_service)
        .add_service(AuthorizationServer::new(Allow {
            tickets: tickets.clone(),
            protected_hosts: Arc::clone(&protected_hosts),
        }))
        .add_service(ExternalProcessorServer::new(Continue {
            tickets,
            protected_hosts,
        }))
        .serve_with_incoming_shutdown(UnixListenerStream::new(listener), stopping);
    let served = drain_until_deadline(server, drain_started).await;
    // Its stop blocks on a join, so it's kept off the async workers.
    if let Some(thread) = rail_center
        && let Err(error) = tokio::task::spawn_blocking(move || thread.stop()).await
    {
        tracing::error!(error = %error, "stopping the Rail Center thread failed");
    }
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = %error, "the gRPC server failed");
            ExitCode::FAILURE
        }
    }
}

// At the deadline the server is dropped; tonic's connection tasks end with the runtime, on exit.
async fn drain_until_deadline(
    server: impl Future<Output = Result<(), tonic::transport::Error>>,
    mut drain_started: oneshot::Receiver<()>,
) -> Result<(), tonic::transport::Error> {
    let mut server = pin!(server);
    let served = tokio::select! {
        served = &mut server => served,
        Ok(()) = &mut drain_started => {
            match tokio::time::timeout(SHUTDOWN_DEADLINE, &mut server).await {
                Ok(served) => served,
                Err(_) => {
                    tracing::warn!(deadline_seconds = SHUTDOWN_DEADLINE.as_secs(), "shut down at the deadline: the connections still open are cut");
                    return Ok(());
                }
            }
        }
    };
    // It ends without an error only once drained, which may be in the poll that starts the drain.
    if served.is_ok() {
        tracing::info!("shut down: every connection drained");
    }
    served
}

// The ticket is fetched and refreshed on the Rail Center thread; calls only read it.
fn start_tickets(
    (rail_center, ticket): (RailCenterSettings, TicketSettings),
) -> Result<(RailCenterThread, Arc<TicketHolder>), String> {
    let client = RailCenterClient::new(&rail_center)?;
    let thread = RailCenterThread::start()?;
    let refresh_interval = ticket.refresh_interval;
    let source = RailCenterTickets {
        client,
        settings: ticket,
    };
    let tickets = TicketHolder::start(
        thread.runtime(),
        source,
        refresh_interval,
        Arc::new(SystemTime::now),
    );
    Ok((thread, Arc::new(tickets)))
}

// Each setting as used, then each RAIL_ variable nothing read, likely a typo; never its value.
fn log_settings(described: &Described, variables: &Variables, unread: &[String]) {
    for entry in described.entries() {
        let source = if variables.is_set(entry.name) {
            "env"
        } else {
            "default"
        };
        match entry.level {
            Level::Info => {
                tracing::info!(name = %entry.name, value = %entry.value, source = %source, "setting");
            }
            Level::Warn(weakens) => {
                tracing::warn!(name = %entry.name, value = %entry.value, source = %source, weakens, "setting");
            }
        }
    }
    for name in unread {
        tracing::warn!(name = %name, "set but not read");
    }
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
        .set_service_status(AuthorizationServer::<Allow>::NAME, status)
        .await;
    health
        .set_service_status(ExternalProcessorServer::<Continue>::NAME, status)
        .await;
}

async fn terminated(mut terminate: Signal) {
    tokio::select! {
        _ = terminate.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

// A connection alone proves nothing: the kernel completes it from the backlog of a hung process.
async fn probe(socket: &Path) -> ExitCode {
    match tokio::time::timeout(PROBE_TIMEOUT, check_health(socket)).await {
        Ok(Ok(WireStatus::Serving)) => {
            println!("serving");
            ExitCode::SUCCESS
        }
        Ok(Ok(status)) => {
            println!("not serving: {}", status.as_str_name());
            ExitCode::FAILURE
        }
        Ok(Err(error)) => {
            println!(
                "can't check health on {}: {}",
                sanitize(&socket.display().to_string()),
                sanitize(&error)
            );
            ExitCode::FAILURE
        }
        Err(_) => {
            println!(
                "no answer on {} within {}s",
                sanitize(&socket.display().to_string()),
                PROBE_TIMEOUT.as_secs()
            );
            ExitCode::FAILURE
        }
    }
}

// The server's health as a whole.
async fn check_health(socket: &Path) -> Result<WireStatus, String> {
    let endpoint = Endpoint::from_shared(format!("unix:{}", socket.display()))
        .map_err(|error| error.to_string())?;
    let channel =
        endpoint
            .connect()
            .await
            .map_err(|error| match std::error::Error::source(&error) {
                Some(cause) => format!("can't connect: {cause}"),
                None => format!("can't connect: {error}"),
            })?;
    let response = HealthClient::new(channel)
        .check(HealthCheckRequest::default())
        .await
        .map_err(|status| format!("{:?}: {}", status.code(), status.message()))?;
    let status = response.into_inner().status;
    // A value this version doesn't know isn't SERVING.
    Ok(WireStatus::try_from(status).unwrap_or(WireStatus::Unknown))
}
