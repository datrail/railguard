# rail-runtime

What a running host does around the rules language: it reads the host's settings, talks to Rail Center, and keeps what
it fetched usable while Rail Center can't be reached. Both hosts, the gateway and the proxy, are built on it, so they
behave the same. See the [main README](../../README.md) for the system around it.

## Entry points

- **Settings:** `Settings` reads typed settings from `Variables`, collecting every refusal so an operator fixes them in
  one round; each `read_…` function reads one group. The settings themselves are in
  [`CONFIGURATION.md`](../../CONFIGURATION.md).
- **Rail Center:** `RailCenterClient`, the one client every Rail Center call goes through, on the `RailCenterThread`, a
  runtime of its own, so a burst of calls can't delay a refresh and a refresh's work can't stall a call.
- **The holders:** `BundleHolder`, `KeySetHolder` and `TicketHolder` keep the policy bundle, the signing keys and the
  ticket through Rail Center failures, refreshed on a jittered schedule with a backoff.
- **The gateway:** `Gateway::handle` judges a call against the bundle held, by its deadline: forward it, deny it (403),
  or refuse it as one the rules can't apply to (503). `DenialReporter` reports each denial without the call waiting.
- **The proxy:** `XRailHeaders` are the proxy's changes to a request, for a protected host or any other.
- **The log:** `logging` writes both hosts' log to stderr, each field quoted so a call's text can't forge a line.

## What it doesn't do

- **Nothing of Envoy, Apigee or gRPC:** each host's own crate adds them, with
  [`rail-grpc-host`](../rail-grpc-host/README.md) for what both gRPC hosts share.
- **No judging of its own:** the rules are [`rail-policy`](../rail-policy/README.md)'s.
