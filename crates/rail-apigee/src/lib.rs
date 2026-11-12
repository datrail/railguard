//! The Apigee host, in the gateway role: Apigee's ExternalCallout service over TCP, each request
//! judged against the policy bundle held.
//!
//! `rail-apigee gateway` (`rail gateway` in its image) serves, with `grpc.health.v1` on the same
//! port; `rail-apigee probe [--ready]` asks it for liveness or readiness, and exits 0 if it
//! answers SERVING.
//!
//! The binary as a library: `run`, and the configuration and services it serves, which its tests
//! drive.

mod callout;
mod config;
mod health;

use std::net::{Ipv4Addr, SocketAddr};
use std::process::ExitCode;

use apigee_callout_proto::apigee::external_callout_service_server::ExternalCalloutServiceServer;
use rail_grpc_host::{Address, Command, Role, Statuses};
use rail_runtime::{GatewaySettings, Settings, Startup, Variables, start_gateway};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot;
use tokio_stream::wrappers::TcpListenerStream;
use tonic_health::pb::health_server::HealthServer;

use crate::callout::SEND_HEADROOM_BYTES;
pub use crate::callout::{Callout, MAX_ANSWERED_BYTES, build_answer};
pub use crate::config::{Config, read_port};
pub use crate::health::check_gateway;

/// The binary: serves as the role named, or with `probe [--ready]`, asks the server on the port
/// configured.
pub async fn run() -> ExitCode {
    rail_runtime::logging::init();
    let command_line: Vec<String> = std::env::args().collect();
    let command = match rail_grpc_host::parse_command(&[Role::Gateway], &command_line) {
        Ok(command) => command,
        Err(usage) => {
            rail_runtime::logging::log_usage(&usage);
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
        // Only the port: the rest is the server's to judge, and the probe names no role.
        Command::Probe { ready } => {
            let port = read_port(&mut settings);
            let (port, _) = settings
                .finish("RAIL_", port)
                .unwrap_or_else(|errors| errors.exit());
            let local = Address::Tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
            rail_grpc_host::probe(&local, rail_grpc_host::probe_service(ready)).await
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
    let listener = match TcpListener::bind((Ipv4Addr::UNSPECIFIED, config.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(error = %error, port = config.port, "can't listen on the port");
            // The port is configuration, so it fails as a refused configuration does.
            return ExitCode::from(rail_runtime::EXIT_CODE);
        }
    };
    let settings = GatewaySettings {
        max_body_bytes: config.max_body_bytes,
        ..GatewaySettings::default()
    };
    let started = config
        .rail_center
        .map(|rail_center| start_gateway(rail_center, settings))
        .transpose();
    let started = match started {
        Ok(started) => started,
        Err(error) => {
            tracing::error!(error = %error, "can't start fetching from Rail Center");
            return ExitCode::FAILURE;
        }
    };
    let (rail_center, gateway) = started.unzip();
    let callout = Callout {
        gateway: gateway.clone(),
    };
    let (statuses, readiness) = Statuses::start(&[], vec![(role, check_gateway(gateway))]).await;
    tracing::info!(port = config.port, "serving the ExternalCallout service");
    let (draining, drain_started) = oneshot::channel();
    let stopping = async {
        rail_grpc_host::terminated(terminate).await;
        statuses.stop().await;
        let _ = draining.send(());
    };
    let callout = ExternalCalloutServiceServer::new(callout)
        .max_decoding_message_size(config.max_message_bytes)
        .max_encoding_message_size(config.max_message_bytes + SEND_HEADROOM_BYTES);
    let server = tonic::transport::Server::builder()
        .add_service(HealthServer::new(readiness))
        .add_service(callout)
        .serve_with_incoming_shutdown(TcpListenerStream::new(listener), stopping);
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
