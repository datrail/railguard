//! The parts of a gRPC host process both binaries share: waiting for SIGTERM, draining the server
//! until a deadline, and the probe, a health check over the server's own socket.

use std::path::Path;
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

// A connection alone proves nothing: the kernel completes it from the backlog of a hung process.
pub async fn probe(socket: &Path) -> ExitCode {
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
