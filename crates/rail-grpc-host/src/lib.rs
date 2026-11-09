//! The parts of a gRPC host process both binaries share: waiting for SIGTERM, draining the server
//! until a deadline, and the probe, a health check over the server's own socket.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::pin;
use std::process::ExitCode;
use std::time::Duration;

use rail_policy::sanitize;
use tokio::signal::unix::Signal;
use tokio::sync::oneshot;
use tonic::transport::Endpoint;
use tonic_health::pb::HealthCheckRequest;
use tonic_health::pb::health_check_response::ServingStatus as WireStatus;
use tonic_health::pb::health_client::HealthClient;

// Envoy keeps its connections open, and an ext_proc stream lasts as long as its request.
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);

// Longer than any answer from a live server, and within the e2e health check's timeout.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

// At the deadline the server is dropped; tonic's connection tasks end with the runtime, on exit.
pub async fn drain_until_deadline(
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

pub async fn terminated(mut terminate: Signal) {
    tokio::select! {
        _ = terminate.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

/// Where a server listens, for the probe to ask it.
#[derive(Clone, Debug)]
pub enum Address {
    Unix(PathBuf),
    Tcp(SocketAddr),
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Unix(socket) => write!(f, "{}", socket.display()),
            Address::Tcp(address) => write!(f, "{address}"),
        }
    }
}

/// Asks the server at `address` for `service`'s health, `""` being the server as a whole; prints
/// the answer, and exits 0 only on SERVING.
// A connection alone proves nothing: the kernel completes it from the backlog of a hung process.
pub async fn probe(address: &Address, service: &str) -> ExitCode {
    match tokio::time::timeout(PROBE_TIMEOUT, check_health(address, service)).await {
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
                sanitize(&address.to_string()),
                sanitize(&error)
            );
            ExitCode::FAILURE
        }
        Err(_) => {
            println!(
                "no answer on {} within {}s",
                sanitize(&address.to_string()),
                PROBE_TIMEOUT.as_secs()
            );
            ExitCode::FAILURE
        }
    }
}

async fn check_health(address: &Address, service: &str) -> Result<WireStatus, String> {
    let uri = match address {
        Address::Unix(socket) => format!("unix:{}", socket.display()),
        Address::Tcp(address) => format!("http://{address}"),
    };
    let endpoint = Endpoint::from_shared(uri).map_err(|error| error.to_string())?;
    let channel =
        endpoint
            .connect()
            .await
            .map_err(|error| match std::error::Error::source(&error) {
                Some(cause) => format!("can't connect: {cause}"),
                None => format!("can't connect: {error}"),
            })?;
    let response = HealthClient::new(channel)
        .check(HealthCheckRequest {
            service: service.to_owned(),
        })
        .await
        .map_err(|status| format!("{:?}: {}", status.code(), status.message()))?;
    let status = response.into_inner().status;
    // A value this version doesn't know isn't SERVING.
    Ok(WireStatus::try_from(status).unwrap_or(WireStatus::Unknown))
}
