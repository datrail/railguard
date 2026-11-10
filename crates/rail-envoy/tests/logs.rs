use std::process::Stdio;
use std::time::{Duration, Instant};

use rail_test_support::Server;

use crate::common::{TempSocket, rail_envoy};

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
    let child = rail_envoy(&socket)
        .env("RAIL_TYPO\nfake", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut server = Server(child);
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
