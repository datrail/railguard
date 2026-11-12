use rail_grpc_host::{Address, ask_health};
use rail_test_support::stdout;

use crate::common::{TempSocket, probe, probe_ready, start_serving};

// The probe itself is tested in rail-grpc-host; this is the binary answering it, here with Rail
// Center disabled, so ready at once.
#[test]
fn it_answers_liveness_and_readiness_until_it_is_dropped() {
    let socket = TempSocket::new("health");
    let server = start_serving(&socket.path);

    let live = probe(&socket.path);
    assert_eq!(live.status.code(), Some(0));
    assert_eq!(stdout(&live), "serving\n");
    let ready = probe_ready(&socket.path);
    assert_eq!(ready.status.code(), Some(0));
    assert_eq!(stdout(&ready), "serving (proxy: disabled)\n");
    // The names Envoy's config gives each callout's health check.
    let address = Address::Unix(socket.path.clone());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for service in [
        "envoy.service.auth.v3.Authorization",
        "envoy.service.ext_proc.v3.ExternalProcessor",
    ] {
        let answer = runtime.block_on(ask_health(&address, service));
        assert_eq!(answer, Ok("serving".to_owned()), "{service}");
    }

    // Dropping it kills it, so no test leaves a server running.
    drop(server);
    assert!(!probe(&socket.path).status.success());
}
