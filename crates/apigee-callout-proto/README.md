# apigee-callout-proto

The interface Apigee uses to hand a request to a service of ours before forwarding it, as Rust code: Apigee's
ExternalCallout service, `apigee.ExternalCalloutService`, its messages, server and client. Generated from the published
definition and kept apart, so `rail-apigee` holds only its own logic and a build needs no code generator. See the
[main README](../../README.md) for the system around it.

## Who uses it

`rail-apigee` serves it, and its tests call it. The apigee stack's stand-in generates its own Python stubs from the same
`.proto`.

## Entry points

One module, `apigee`, not re-exported at the root, so its `Request` and `Response` don't clash with tonic's:

- **`MessageContext`**, what Apigee sends and the callout answers, with its `Request`, `FlowVariable` and `Strings`;
- **`external_callout_service_server`**, the service a callout implements;
- **`external_callout_service_client`**, to call one, as tests do.

## Regenerating it

`proto/external_callout.proto` is vendored from
[external-callout-samples](https://github.com/srinandan/external-callout-samples) (Apache-2.0), unchanged below the
header naming its commit. `src/apigee.rs` is generated from it and committed: after changing the `.proto`, run
`make proto` and commit both.

The generator, `codegen/`, is outside the workspace, so its dependencies stay out of the build; bump its `prost-build`
and `tonic-prost-build` with the workspace's `prost` and `tonic`, so the code matches what compiles.

## What it doesn't do

No logic: what a request means, and what to answer, is `rail-apigee`'s.
