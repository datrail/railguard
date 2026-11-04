//! The Envoy host, in the proxy role: ext_authz and ext_proc on a unix socket Envoy shares. For
//! now it allows every request unchanged.
//!
//! `rail-envoy` serves; `rail-envoy probe` exits 0 if the server accepts a connection, for a
//! container health check in an image with no shell.

use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::Path;
use std::pin::Pin;
use std::process::ExitCode;

use envoy_types::ext_authz::v3::CheckResponseExt;
use envoy_types::ext_authz::v3::pb::{
    Authorization, AuthorizationServer, CheckRequest, CheckResponse,
};
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::{
    ExternalProcessor, ExternalProcessorServer,
};
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_response::Response;
use envoy_types::pb::envoy::service::ext_proc::v3::{
    BodyResponse, HeadersResponse, ProcessingRequest, ProcessingResponse, TrailersResponse,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio_stream::wrappers::UnixListenerStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{Status, Streaming};

const SOCKET: &str = "/run/rail/ext.sock";

#[tokio::main]
async fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        None => serve(Path::new(SOCKET)).await,
        Some("probe") => probe(Path::new(SOCKET)).await,
        Some(_) => {
            eprintln!("usage: rail-envoy [probe]");
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
            eprintln!("{}: {error}", socket.display());
            return ExitCode::from(2);
        }
    };
    let served = tonic::transport::Server::builder()
        .add_service(AuthorizationServer::new(Allow))
        .add_service(ExternalProcessorServer::new(Continue))
        .serve_with_incoming_shutdown(UnixListenerStream::new(listener), terminated(terminate))
        .await;
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("the server stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

// A socket left by an earlier run is replaced; anything else at the path is refused.
fn bind(socket: &Path) -> Result<UnixListener, String> {
    match fs::symlink_metadata(socket) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(socket).map_err(|error| format!("can't replace it: {error}"))?;
        }
        Ok(_) => return Err("exists and isn't a socket".to_owned()),
        Err(_) => {}
    }
    let listener = UnixListener::bind(socket).map_err(|error| format!("can't bind: {error}"))?;
    // Envoy runs in this socket's group, as another user.
    fs::set_permissions(socket, fs::Permissions::from_mode(0o660))
        .map_err(|error| format!("can't share it: {error}"))?;
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
            eprintln!("{}: {error}", socket.display());
            ExitCode::FAILURE
        }
    }
}

struct Allow;

#[tonic::async_trait]
impl Authorization for Allow {
    async fn check(
        &self,
        _request: tonic::Request<CheckRequest>,
    ) -> Result<tonic::Response<CheckResponse>, Status> {
        Ok(tonic::Response::new(CheckResponse::with_status(
            Status::ok(""),
        )))
    }
}

struct Continue;

type ProcessStream = Pin<Box<dyn Stream<Item = Result<ProcessingResponse, Status>> + Send>>;

#[tonic::async_trait]
impl ExternalProcessor for Continue {
    type ProcessStream = ProcessStream;

    async fn process(
        &self,
        request: tonic::Request<Streaming<ProcessingRequest>>,
    ) -> Result<tonic::Response<ProcessStream>, Status> {
        let replies = request.into_inner().map(|message| {
            let request = message?
                .request
                .ok_or_else(|| Status::invalid_argument("the message names no phase"))?;
            Ok(ProcessingResponse {
                response: Some(continue_after(request)),
                ..Default::default()
            })
        });
        Ok(tonic::Response::new(Box::pin(replies)))
    }
}

// Each phase Envoy sends must be answered with its own kind of response.
fn continue_after(request: Request) -> Response {
    match request {
        Request::RequestHeaders(_) => Response::RequestHeaders(HeadersResponse::default()),
        Request::ResponseHeaders(_) => Response::ResponseHeaders(HeadersResponse::default()),
        Request::RequestBody(_) => Response::RequestBody(BodyResponse::default()),
        Request::ResponseBody(_) => Response::ResponseBody(BodyResponse::default()),
        Request::RequestTrailers(_) => Response::RequestTrailers(TrailersResponse::default()),
        Request::ResponseTrailers(_) => Response::ResponseTrailers(TrailersResponse::default()),
    }
}
