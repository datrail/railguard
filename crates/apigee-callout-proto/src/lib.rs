//! Apigee's ExternalCallout service, `apigee.ExternalCalloutService`: the messages, its server
//! and its client, generated from `proto/external_callout.proto` by `make proto` and committed.

// A module, not re-exported at the root, so its `Request` and `Response` don't clash with tonic's.
#[allow(clippy::all, rustdoc::all)]
#[rustfmt::skip]
pub mod apigee;
