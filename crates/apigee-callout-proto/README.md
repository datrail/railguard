# apigee-callout-proto

The interface Apigee uses to hand a request to a service of ours before forwarding it, as Rust code that `rail-apigee` serves. Generated from the published definition and kept apart, so `rail-apigee` holds only its own logic and a build needs no code generator.

Apigee's ExternalCallout service, `apigee.ExternalCalloutService`: its messages, server and client, for `rail-apigee`.

`proto/external_callout.proto` is vendored from [external-callout-samples](https://github.com/srinandan/external-callout-samples) (Apache-2.0), unchanged below the header naming its commit, as the Python gateway vendors it. `src/apigee.rs` is generated from it and committed, so a build needs no code generation: after changing the `.proto`, run `make proto` and commit both. The generator, `codegen/`, is outside the workspace, so its dependencies stay out of the build; bump its `prost-build` and `tonic-prost-build` with the workspace's `prost` and `tonic`, so the code matches what compiles.
