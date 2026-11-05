use std::fs;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::time::Duration;

use crate::common::{Server, TempSocket, probe, rail_envoy};

// The exit code of a run whose configuration is refused.
fn refused_configuration() -> Option<i32> {
    let mut server = Server::spawn(rail_envoy(Path::new("relative.sock")));
    server.wait_for_exit(Duration::from_secs(5)).unwrap()
}

#[test]
fn a_file_at_the_socket_path_is_refused_and_kept() {
    let socket = TempSocket::new("file");
    fs::write(&socket.path, "keep").unwrap();
    let mut server = Server::spawn(rail_envoy(&socket.path));

    let exit = server.wait_for_exit(Duration::from_secs(5));

    assert_eq!(exit, Some(refused_configuration()));
    let logs = server.read_logs();
    assert!(logs.contains("exists and isn't a socket"), "{logs}");
    assert_eq!(fs::read_to_string(&socket.path).unwrap(), "keep");
}

#[test]
fn a_socket_left_by_an_earlier_run_is_replaced() {
    let socket = TempSocket::new("stale");
    drop(UnixListener::bind(&socket.path).unwrap());
    assert!(socket.path.exists());

    let mut server = Server::start_serving(&socket.path);

    assert!(probe(&socket.path).status.success());
    server.terminate();
    assert_eq!(server.wait_for_exit(Duration::from_secs(2)), Some(Some(0)));
}

#[test]
fn an_unknown_argument_is_a_usage_error_and_serves_nothing() {
    let socket = TempSocket::new("usage");
    let mut command = rail_envoy(&socket.path);
    command.arg("serve");
    let mut server = Server::spawn(command);

    let exit = server.wait_for_exit(Duration::from_secs(5));

    assert_eq!(exit, Some(refused_configuration()));
    let logs = server.read_logs();
    assert!(logs.contains("usage: rail-envoy [probe]"), "{logs}");
    assert!(!socket.path.exists());
}
