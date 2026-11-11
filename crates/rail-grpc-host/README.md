# rail-grpc-host

Both hosts are gRPC servers, and both need to answer health checks and to stop cleanly when told to. This crate does that once for both, so they can't differ.

The parts of a gRPC host process that `rail-envoy` and `rail-apigee` share: waiting for SIGTERM, draining the server until a deadline, and the health probe. `rail-runtime` stays free of gRPC.

The probe is tested here; the drain and the SIGTERM wait through `rail-envoy`'s binary (`tests/shutdown.rs`), so give cargo-mutants `--test-package rail-grpc-host --test-package rail-envoy`. `rail-apigee`'s tests don't repeat them.
