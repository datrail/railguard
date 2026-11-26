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
- **The holders:** `BundleHolder`, `KeySetHolder`, `TicketHolder` and `CapturePlanHolder` keep the policy bundle, the
  signing keys, the ticket and the capture plan through Rail Center failures, refreshed on a jittered period: backing
  off after a failure that may pass, and after a refusal too while nothing is held, or at any time for the keys and the
  ticket, which lapse.
- **The gateway:** `Gateway::handle` judges a call against the bundle held, by its deadline: forward it, deny it (403),
  or refuse it as one the rules can't apply to (503). `DenialReporter` reports each denial without the call waiting.
- **The proxy:** `XRailHeaders` are the proxy's changes to a request, for a protected host or any other.
  `CallRecordReporter` sends each closed capture window holding a call or a drop to Rail Center as its call record
  document, oldest first, one at a time, retrying with a backoff; an empty window isn't sent, and a document Rail Center
  refuses (a 4xx other than 401, 403, 408 or 429) is dropped. Besides the oldest, it keeps up to the newest window's
  plan's `max_unsent_windows` and up to two million records and steps, dropping the oldest past either; each drop is
  counted in the next document. `Capture`, a task on the `CaptureThread`'s runtime, counts the calls the callouts hand
  over through a `CaptureSender` in windows under the capture plan held, each closed window to the reporter, sent at
  once when the plan goes: `is_capturing`, a plan held and the task taking calls, decides a call's capture at its
  headers, and `send` never waits, a call the full queue of 4 096 turns away counted as dropped. `send_request` reads a
  request's body as a callout was sent it: whole within its cap and the 256 MiB every body kept shares, else unread, as
  not sent, cut short of its declared length, or past either. Stopping it counts the calls already handed over, closes
  the open window and sends the windows waiting, for up to 2 s. Dropping it does the same, unawaited. `start_proxy`
  starts them all, with the ticket, as `start_gateway` starts the gateway's: `ProxyStop` stops capture, then its
  threads.
- **The log:** `logging` writes both hosts' log to stderr as JSON lines, Cloud Logging's on Cloud Run, or as text under
  `RAIL_LOG_FORMAT=text`, each field quoted or a JSON string, with what hides escaped, so a call's text can't forge a
  line.

## What it doesn't do

- **Nothing of Envoy, Apigee or gRPC:** each host's own crate adds them, with
  [`rail-grpc-host`](../rail-grpc-host/README.md) for what both gRPC hosts share.
- **No judging of its own:** the rules are [`rail-model`](../rail-model/README.md)'s.
