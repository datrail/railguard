use crate::common::{Server, TempSocket, probe};

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

// The probe itself is tested in rail-grpc-host; this is the binary answering it.
#[test]
fn the_probe_passes_on_a_serving_server() {
    let socket = TempSocket::new("serving");
    let _server = Server::start_serving(&socket.path);

    let output = probe(&socket.path);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output), "serving\n");
}
