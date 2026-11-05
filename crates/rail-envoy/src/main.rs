//! The Envoy host, in the proxy role: ext_authz and ext_proc on a unix socket Envoy shares. For
//! now it allows every request, keeping the agent's x-rail headers from the upstream.
//!
//! `rail-envoy` serves; `rail-envoy probe` exits 0 if the server accepts a connection, for a
//! container health check in an image with no shell.

mod services;
mod xrail;

use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use envoy_types::ext_authz::v3::pb::AuthorizationServer;
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessorServer;
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio_stream::wrappers::UnixListenerStream;

use crate::services::{Allow, Continue};

const DEFAULT_SOCKET: &str = "/run/rail/ext.sock";

/// What this process runs with, read in one place so settings can replace the defaults.
struct Config {
    socket: PathBuf,
}

impl Config {
    fn read() -> Self {
        Self {
            socket: PathBuf::from(DEFAULT_SOCKET),
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    // The probe reads the same configuration, so it checks the socket where it's served.
    let config = Config::read();
    match std::env::args().nth(1).as_deref() {
        None => serve(&config.socket).await,
        Some("probe") => probe(&config.socket).await,
        Some(argument) => {
            eprintln!("unknown argument {argument:?}; usage: rail-envoy [probe]");
            ExitCode::from(2)
        }
    }
}

async fn serve(socket: &Path) -> ExitCode {
    // SIGTERM is how a container is stopped; as PID 1 nothing handles it unless we do.
    let terminate = match signal(SignalKind::terminate()) {
        Ok(terminate) => terminate,
        Err(error) => {
            eprintln!("can't handle SIGTERM: {error}");
            return ExitCode::FAILURE;
        }
    };
    let listener = match bind(socket) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    tracing::info!(socket = %socket.display(), "serving Envoy");
    let served = tonic::transport::Server::builder()
        .add_service(AuthorizationServer::new(Allow))
        .add_service(ExternalProcessorServer::new(Continue))
        .serve_with_incoming_shutdown(UnixListenerStream::new(listener), terminated(terminate))
        .await;
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("the gRPC server failed: {error}");
            ExitCode::FAILURE
        }
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

async fn terminated(mut terminate: Signal) {
    tokio::select! {
        _ = terminate.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

async fn probe(socket: &Path) -> ExitCode {
    match UnixStream::connect(socket).await {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("no server on {}: {error}", socket.display());
            ExitCode::FAILURE
        }
    }
}
