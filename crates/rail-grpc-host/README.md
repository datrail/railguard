# rail-grpc-host

Both hosts are gRPC servers, and both need to answer health checks and to stop cleanly when told to. This crate does
that once for both, so they can't differ. See the [main README](../../README.md) for the system around it.

## Entry points

- **`parse_command`** reads the command line, `<role>` or `probe [--ready]`, against the `Role`s a binary carries; a
  role has no default;
- **`Statuses`** and **`Readiness`**, the health service: `""` and the services a host names are liveness; `rail.Ready`
  is readiness, SERVING while every role's `RoleCheck` says it's ready, read on each check, with each role's state in
  the answer's metadata (`rail-ready-<role>`). Once stopping, all are NOT_SERVING for good;
- **`terminated`** waits for SIGTERM or Ctrl-C;
- **`drain_until_deadline`** lets the server finish what it's serving, for at most 5 s, then cuts the connections still
  open;
- **`probe`** asks the server's own `grpc.health.v1` at an `Address`, for an exec probe, and exits 0 only on SERVING,
  printing each role's state the server gives after the status; **`ask_health`** is the same check, its answer returned.

## What it doesn't do

- **Nothing of what makes a role ready:** each host supplies its roles' checks.
- **Nothing of Rail Center:** `rail-runtime` stays free of gRPC, and this crate of Rail Center.

## Testing

The probe and the health service are tested here, each role's states in its host; the drain and the SIGTERM wait through
`rail-envoy`'s binary (`tests/shutdown.rs`), so give cargo-mutants
`--test-package rail-grpc-host --test-package rail-envoy`. `rail-apigee`'s tests don't repeat them.
