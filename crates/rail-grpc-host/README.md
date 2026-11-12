# rail-grpc-host

Both hosts are gRPC servers, and both need to answer health checks and to stop cleanly when told to. This crate does
that once for both, so they can't differ. See the [main README](../../README.md) for the system around it.

## Entry points

- **`parse_command`** reads the command line, `<role>` or `probe [--ready]`, against the `Role`s a binary carries; a
  role has no default;
- **`terminated`** waits for SIGTERM or Ctrl-C;
- **`drain_until_deadline`** lets the server finish what it's serving, for at most 5 s, then cuts the connections still
  open;
- **`probe`** asks the server's own `grpc.health.v1` at an `Address`, for an exec probe, and exits 0 only on SERVING;
  **`ask_health`** is the same check, its answer returned.

## What it doesn't do

- **No service of its own,** and nothing of what each host reports as healthy or ready: that's the host's.
- **Nothing of Rail Center:** `rail-runtime` stays free of gRPC, and this crate of Rail Center.

## Testing

The probe is tested here; the drain and the SIGTERM wait through `rail-envoy`'s binary (`tests/shutdown.rs`), so give
cargo-mutants `--test-package rail-grpc-host --test-package rail-envoy`. `rail-apigee`'s tests don't repeat them.
