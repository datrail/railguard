use rail_grpc_host::{Address, ask_health};
use rail_test_support::stdout;

use crate::common::{TempSocket, probe, rail_envoy, serve, start_serving, with_settings};

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

// The names Envoy's config gives each callout's health check.
#[test]
fn liveness_answers_for_ext_authz_and_ext_proc() {
    let socket = TempSocket::new("live-services");
    let _server = start_serving(&socket.path);
    let address = Address::Unix(socket.path.clone());
    let runtime = tokio::runtime::Runtime::new().unwrap();

    for service in [
        "envoy.service.auth.v3.Authorization",
        "envoy.service.ext_proc.v3.ExternalProcessor",
    ] {
        let answer = runtime.block_on(ask_health(&address, service));
        assert_eq!(answer, Ok("serving".to_owned()), "{service}");
    }
}

fn probe_ready(socket: &std::path::Path) -> std::process::Output {
    with_settings(socket)
        .args(["probe", "--ready"])
        .output()
        .unwrap()
}

#[test]
fn with_rail_center_disabled_it_is_ready_at_once() {
    let socket = TempSocket::new("ready-disabled");
    let _server = start_serving(&socket.path);

    let output = probe_ready(&socket.path);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output), "serving (proxy: disabled)\n");
}

// The binary's readiness is the ticket's; the states are tested in readiness.rs.
#[test]
fn with_no_ticket_it_is_live_but_not_ready() {
    let socket = TempSocket::new("ready-no-ticket");
    let mut command = rail_envoy(&socket.path);
    command.envs([
        ("RAIL_DISABLED", "false"),
        ("RAIL_CENTER_URL", "http://127.0.0.1:1"),
        ("RAIL_CENTER_ALLOW_HTTP", "true"),
        ("RAIL_HOST_ID", "host-1"),
        ("RAIL_SANDBOX_NAME", "sandbox-1"),
        ("RAIL_PROXY_PROTECTED_HOSTS", "mcp.example.com"),
    ]);
    let _server = serve(command, &socket.path);

    let output = probe_ready(&socket.path);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stdout(&output),
        "not serving: NOT_SERVING (proxy: issuer-unreachable)\n"
    );
    assert_eq!(stdout(&probe(&socket.path)), "serving\n");
}
