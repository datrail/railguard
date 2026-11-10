use rail_test_support::stdout;

use crate::common::{TempSocket, probe, start_serving};

// The probe itself is tested in rail-grpc-host; this is the binary answering it.
#[test]
fn the_probe_passes_on_a_serving_server_and_fails_once_it_is_dropped() {
    let socket = TempSocket::new("serving");
    let server = start_serving(&socket.path);

    let output = probe(&socket.path);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output), "serving\n");
    // Dropping it kills it, so no test leaves a server running.
    drop(server);
    assert!(!probe(&socket.path).status.success());
}
