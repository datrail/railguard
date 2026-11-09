# rail-grpc-host

The parts of a gRPC host process that `rail-envoy` and `rail-apigee` share: waiting for SIGTERM, draining the server until a deadline, and the health probe. `rail-runtime` stays free of gRPC.

Its behaviour is tested through `rail-envoy`'s binary (`tests/health.rs`, `tests/shutdown.rs`); give cargo-mutants `--test-package rail-envoy`. `rail-apigee`'s tests leave it to them.
