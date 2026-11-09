# apigee-callout-proto

Apigee's ExternalCallout service, `apigee.ExternalCalloutService`: its messages, server and client, for `rail-apigee`.

`proto/external_callout.proto` is vendored from [external-callout-samples](https://github.com/srinandan/external-callout-samples) (Apache-2.0), unchanged below the header naming its commit, as the Python gateway vendors it. `src/apigee.rs` is generated from it and committed, so a build needs no code generation: after changing the `.proto`, run `make proto` and commit both. The generator, `codegen/`, is outside the workspace, so its dependencies stay out of the build.
