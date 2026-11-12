use std::fs;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::time::Duration;

use rail_test_support::Server;

use crate::common::{TempSocket, probe, rail_envoy, start_serving, with_settings};

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

    let mut server = start_serving(&socket.path);

    assert!(probe(&socket.path).status.success());
    server.terminate();
    assert_eq!(server.wait_for_exit(Duration::from_secs(2)), Some(Some(0)));
}

// The role has no default, and a role this binary doesn't carry is refused like any argument.
#[test]
fn with_no_role_or_another_binarys_it_serves_nothing() {
    for (arguments, expected) in [
        (
            &[][..],
            "no role given; usage: rail-envoy proxy | rail-envoy probe [--ready]",
        ),
        (
            &["gateway"][..],
            r#"unknown arguments ["gateway"]; usage: rail-envoy proxy | rail-envoy probe [--ready]"#,
        ),
    ] {
        let socket = TempSocket::new("no-role");
        let mut command = with_settings(&socket.path);
        command.args(arguments);
        let mut server = Server::spawn(command);

        let exit = server.wait_for_exit(Duration::from_secs(5));

        assert_eq!(exit, Some(refused_configuration()), "{arguments:?}");
        let logs = server.read_logs();
        assert!(logs.contains(expected), "{logs}");
        assert!(!logs.contains("starting"), "{logs}");
        assert!(!socket.path.exists());
    }
}

// It reads only the socket, so a setting only the server reads, refused there, can't fail it.
#[test]
fn the_probe_reads_only_the_socket() {
    let socket = TempSocket::new("probe-settings");
    let _server = start_serving(&socket.path);
    let mut command = with_settings(&socket.path);
    command
        .arg("probe")
        .env("RAIL_DISABLED", "false")
        .env("RAIL_LOG_LEVEL", "loud")
        .env("RAIL_TYPO", "x");

    let output = command.output().unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}
