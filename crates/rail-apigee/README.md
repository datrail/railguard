# rail-apigee

The gateway, for MCP servers behind Apigee. Apigee hands it each request on its way to an MCP server; rail-apigee judges
the call against Rail Center's rules, using the caller's ticket, and tells Apigee to forward it or to refuse it. It
reports each refusal to Rail Center, so people can see what was stopped and why.

The Apigee host: Apigee's ExternalCallout service over TCP on `RAIL_CALLOUT_PORT`. It judges each `POST` against the
policy bundle held, and answers with what Apigee should change: the received context, with every `x-rail` and `x-rail-*`
header removed, and every name holding `_` (some upstreams read it as `-`), and the flow variable `rail.decision` set to
`allow` or `refuse`, plus `rail.status` and `rail.body` for a refusal. Nothing else in the context changes. Under
`RAIL_DISABLED=true` it allows every call it can answer (below).

A request past 4 MiB less 1 KiB is refused unjudged, 503, with a warning logged: Apigee takes a callout's answer of at
most 4 MiB, so it would fail the answer anyway.

It serves `grpc.health.v1` on the same port: `""` is liveness, SERVING while it serves; `datrail.gateway.Ready` is
readiness, SERVING once a bundle is held or with Rail Center disabled. Both are NOT_SERVING once it's stopping.
`rail-apigee probe [--ready]` (`rail probe [--ready]` in its image) asks it, for an exec probe, and exits 0 only on
SERVING.

On SIGTERM or Ctrl-C it drains for at most 5 s, then cuts the connections still open; with Rail Center on, its thread
then has at most 1 s more to stop.

## Configuration

Every setting it reads, its own and those both hosts share, is in
[`CONFIGURATION.md`](../../CONFIGURATION.md#rail-apigee).

## The image

`docker build -f crates/rail-apigee/Dockerfile .` builds it, from the repository root: distroless, as uid 10001, serving
on 8080. The binary is `/usr/local/bin/rail`: for a health check, run `rail probe`, or `rail probe --ready` for
readiness.

## The Apigee proxy

`e2e/apigee/apiproxy/` is a proxy that does what rail-apigee needs; [the apigee stack](../../e2e/apigee/README.md) runs
it on a stand-in for Apigee. Its `__…__` placeholders are the deployment's. What it must do, and what's left to the
deployment:

- **`EC-Rail`**, an ExternalCallout policy in the request PreFlow, sending the request's headers and content
  (`with.request.headers` and `with.request.content`), and nothing of the response. Request streaming stays off: a
  streamed request reaches the callout with no content.
- **`TimeoutMs` well above 5000**, rail-apigee's deadline for a call, which starts once the request has arrived, so its
  answer arrives first; the stand-in uses 6000.
- **`RF-Refuse`** when `rail.decision` is `refuse`: a RaiseFault answering `rail.status` with `rail.body`.
- **Failing closed:** a FaultRule on `externalcallout.EC-Rail.failed`, and a step on `rail.decision = null`, both
  raising `RF-CalloutFailed`'s 503, so a callout that's down, times out, or answers without a decision forwards nothing.
  The FaultRule names EC-Rail, so another policy's fault keeps its own error.
- **The 4 MiB answer:** Apigee takes a callout answer of at most 4 MiB, and the answer must echo the body (an empty
  `content` would remove it), so a request near or over 4 MiB is refused, 503, whatever
  `RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB` allows.
- **Apigee's 10 MB request:** Apigee answers a request body past 10 MB itself, 413 `protocol.http.TooBigBody`, before
  the callout (seen live). Streaming would lift it, but must stay off: a streamed request reaches the callout with no
  content, judged as a call that names nothing.
- **The callout's own credential:** `EC-Rail` sends a Google ID token for a callout on Cloud Run; remove its
  `<Authentication>` otherwise. The target has none, so the agent's `Authorization` reaches the MCP server as sent.
