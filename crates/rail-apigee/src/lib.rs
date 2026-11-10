//! The Apigee host, in the gateway role: Apigee's ExternalCallout service over TCP, each request
//! judged against the policy bundle held.
//!
//! `rail-apigee` (`rail` in its image) serves, with `grpc.health.v1` on the same port; `rail-apigee probe [--ready]`
//! asks it for liveness or readiness, and exits 0 if it answers SERVING.
//!
//! The binary as a library: `run`, and the configuration and services it serves, which its tests
//! drive.

mod callout;
mod config;
mod health;

use std::net::{Ipv4Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::SystemTime;

use apigee_callout_proto::apigee::external_callout_service_server::ExternalCalloutServiceServer;
use rail_grpc_host::Address;
use rail_runtime::{
    BundleHolder, BundleSettings, Clock, DenialReporter, Gateway, GatewaySettings, Jitter,
    KeySetHolder, RailCenterBundles, RailCenterClient, RailCenterDenials, RailCenterKeys,
    RailCenterSettings, RailCenterThread, Settings, Startup, Variables,
};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot;
use tokio_stream::wrappers::TcpListenerStream;
use tonic_health::pb::health_server::HealthServer;

pub use crate::callout::{Callout, build_answer, read_path};
pub use crate::config::Config;
pub use crate::health::{READY, Readiness, Statuses};

// The answer echoes the content, plus our flow variables and removals: a few hundred bytes.
const SEND_HEADROOM_BYTES: usize = 1024;

/// The binary: serves, or with `probe [--ready]`, asks the server on the port configured.
pub async fn run() -> ExitCode {
    rail_runtime::logging::init();
    // The probe reads the same configuration, so it asks the port where it's served.
    let variables = Variables::from_process();
    let mut settings = Settings::new(&variables);
    let config = Config::read(&mut settings);
    let (config, startup) = settings
        .finish("RAIL_", config)
        .unwrap_or_else(|errors| errors.exit());
    rail_runtime::logging::set_level(config.log_level);
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let local = Address::Tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, config.port)));
    match arguments.as_slice() {
        [] => serve(config, &startup).await,
        ["probe"] => rail_grpc_host::probe(&local, "").await,
        ["probe", "--ready"] => rail_grpc_host::probe(&local, READY).await,
        _ => {
            let program = rail_grpc_host::program_name();
            eprintln!("unknown arguments {arguments:?}; usage: {program} [probe [--ready]]");
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
    let listener = match TcpListener::bind((Ipv4Addr::UNSPECIFIED, config.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(error = %error, port = config.port, "can't listen on the port");
            // The port is configuration, so it fails as a refused configuration does.
            return ExitCode::from(rail_runtime::EXIT_CODE);
        }
    };
    let started = match config.rail_center.map(start_gateway).transpose() {
        Ok(started) => started,
        Err(error) => {
            tracing::error!(error = %error, "can't start fetching from Rail Center");
            return ExitCode::FAILURE;
        }
    };
    let (rail_center, bundles, gateway) = match started {
        Some(started) => (
            Some(started.thread),
            Some(started.bundles),
            Some(started.gateway),
        ),
        None => (None, None, None),
    };
    let callout = Callout { gateway };
    let (statuses, readiness) = Statuses::start(bundles).await;
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

struct Started {
    thread: RailCenterThread,
    bundles: Arc<BundleHolder>,
    gateway: Arc<Gateway>,
}

// The bundle, the key set and the denial reports go through the Rail Center thread; calls only
// read what's held.
fn start_gateway(
    (rail_center, bundle): (RailCenterSettings, BundleSettings),
) -> Result<Started, String> {
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
        Arc::clone(&bundles),
        key_sets,
        reporter,
        Arc::new(SystemTime::now),
        GatewaySettings::default(),
    );
    Ok(Started {
        thread,
        bundles,
        gateway: Arc::new(gateway),
    })
}
