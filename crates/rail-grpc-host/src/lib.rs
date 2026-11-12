//! The parts of a gRPC host process both binaries share: the command line, the health service,
//! waiting for SIGTERM, draining the server until a deadline, and the probe, a health check on the
//! server's own address.

mod command;
mod health;

use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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

pub use crate::command::{Command, Role, parse_command};
pub use crate::health::{READY, Readiness, RoleCheck, RoleState, Statuses, breakdown_header};

// Envoy keeps its connections open, and an ext_proc stream lasts as long as its request.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);

// Longer than any answer from a live server, and within the e2e health check's timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

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

/// The name the binary was run under, for its usage line: `rail` in its image, the crate's from
/// cargo.
pub fn program_name() -> String {
    let program = std::env::args_os().next().unwrap_or_default();
    let name = Path::new(&program).file_name().unwrap_or_default();
    sanitize(&name.to_string_lossy()).to_string()
}

/// The service `rail probe` asks for: readiness with `--ready`, else the server as a whole.
pub fn probe_service(ready: bool) -> &'static str {
    if ready { READY } else { "" }
}

/// Asks the server at `address` for `service`'s health, `""` being the server as a whole; prints
/// the answer, and exits 0 only on SERVING.
pub async fn probe(address: &Address, service: &str) -> ExitCode {
    match ask_health(address, service).await {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(line) => {
            println!("{line}");
            ExitCode::FAILURE
        }
    }
}

/// The line [`probe`] prints: `Ok` only on SERVING, each role's state after the status when the
/// server gives them, e.g. `not serving: NOT_SERVING (proxy: expired)`.
// A connection alone proves nothing: the kernel completes it from the backlog of a hung process.
pub async fn ask_health(address: &Address, service: &str) -> Result<String, String> {
    match tokio::time::timeout(PROBE_TIMEOUT, check_health(address, service)).await {
        Ok(Ok((WireStatus::Serving, roles))) => Ok(format!("serving{}", format_roles(&roles))),
        Ok(Ok((status, roles))) => Err(format!(
            "not serving: {}{}",
            status.as_str_name(),
            format_roles(&roles)
        )),
        Ok(Err(error)) => Err(format!(
            "can't check health on {}: {}",
            sanitize(&address.to_string()),
            sanitize(&error)
        )),
        Err(_) => Err(format!(
            "no answer on {} within {}s",
            sanitize(&address.to_string()),
            PROBE_TIMEOUT.as_secs()
        )),
    }
}

// ` (gateway: held, proxy: valid)`, by role name; nothing for a server that gives none.
fn format_roles(roles: &[(String, String)]) -> String {
    if roles.is_empty() {
        return String::new();
    }
    let roles: Vec<String> = roles
        .iter()
        .map(|(role, state)| format!("{}: {}", sanitize(role), sanitize(state)))
        .collect();
    format!(" ({})", roles.join(", "))
}

async fn check_health(
    address: &Address,
    service: &str,
) -> Result<(WireStatus, Vec<(String, String)>), String> {
    let uri = match address {
        Address::Unix(socket) => format!("unix:{}", socket.display()),
        Address::Tcp(address) => format!("http://{address}"),
    };
    let endpoint = Endpoint::from_shared(uri).map_err(|error| error.to_string())?;
    let channel = endpoint
        .connect()
        .await
        // The innermost cause: over TCP, tonic wraps the IO error twice.
        .map_err(|error| {
            match std::iter::successors(error.source(), |&cause| cause.source()).last() {
                Some(cause) => format!("can't connect: {cause}"),
                None => format!("can't connect: {error}"),
            }
        })?;
    let response = HealthClient::new(channel)
        .check(HealthCheckRequest {
            service: service.to_owned(),
        })
        .await
        .map_err(|status| format!("{:?}: {}", status.code(), status.message()))?;
    let mut roles: Vec<(String, String)> = response
        .metadata()
        .clone()
        .into_headers()
        .iter()
        .filter_map(|(name, value)| {
            let role = name.as_str().strip_prefix(health::BREAKDOWN_PREFIX)?;
            Some((
                role.to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            ))
        })
        .collect();
    roles.sort();
    let status = response.into_inner().status;
    // A value this version doesn't know isn't SERVING.
    Ok((
        WireStatus::try_from(status).unwrap_or(WireStatus::Unknown),
        roles,
    ))
}
