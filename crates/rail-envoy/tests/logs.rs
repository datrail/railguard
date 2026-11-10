use std::time::{Duration, Instant};

use rail_test_support::Server;

use crate::common::{TempSocket, rail_envoy, serve};

// The fields after the target, of the one line holding `needle`: the first ": " ends the target.
fn find_fields<'a>(logs: &'a str, needle: &str) -> &'a str {
    let lines: Vec<_> = logs.lines().filter(|line| line.contains(needle)).collect();
    assert_eq!(lines.len(), 1, "{logs}");
    let (_, fields) = lines[0]
        .split_once(": ")
        .unwrap_or_else(|| panic!("{logs}"));
    fields
}

// The real binary, so it fails if `init` doesn't install the formatter.
#[test]
fn the_log_escapes_a_setting_value_and_an_unread_name() {
    let temp = TempSocket::new("logs");
    let dir = temp.path.parent().unwrap();
    let socket = dir.join("e\u{1b}[31m\"x.sock");
    let mut command = rail_envoy(&socket);
    command.env("RAIL_TYPO\nfake", "1");
    let mut server = Server::spawn(command);
    // Bound only once SIGTERM is handled.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "the server never bound");
        std::thread::sleep(Duration::from_millis(20));
    }

    server.terminate();

    assert_eq!(server.wait_for_exit(Duration::from_secs(2)), Some(Some(0)));
    let logs = server.read_logs();
    assert_eq!(
        find_fields(&logs, "name=\"RAIL_EXT_SOCKET\""),
        format!(
            r#"setting name="RAIL_EXT_SOCKET" value="{}/e\u{{1b}}[31m\"x.sock" source="env""#,
            dir.display()
        )
    );
    assert_eq!(
        find_fields(&logs, "set but not read"),
        r#"set but not read name="RAIL_TYPO\u{a}fake""#
    );
}

// The level is read with the other settings, so it applies only once they're read.
#[test]
fn at_warn_the_binary_logs_its_warnings_and_no_info_line() {
    let socket = TempSocket::new("warn");
    let mut command = rail_envoy(&socket.path);
    command.env("RAIL_LOG_LEVEL", "warn");
    let mut server = serve(command, &socket.path);

    server.terminate();

    assert_eq!(server.wait_for_exit(Duration::from_secs(2)), Some(Some(0)));
    let logs = server.read_logs();
    assert!(
        logs.lines()
            .any(|line| line.contains(" WARN ") && line.contains(r#"name="RAIL_DISABLED""#)),
        "{logs}"
    );
    assert!(!logs.contains(" INFO "), "{logs}");
}
