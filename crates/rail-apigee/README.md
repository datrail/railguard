# rail-apigee

The Apigee host: Apigee's ExternalCallout service over TCP on `RAIL_GATEWAY_PORT` (default 8080). It judges each `POST` against the policy bundle held, and answers with what Apigee should change: the received context, with every `x-rail` and `x-rail-*` header removed, and every name holding `_` (some upstreams read it as `-`), and the flow variable `rail.decision` set to `allow` or `refuse`, plus `rail.status` and `rail.body` for a refusal. Nothing else in the context changes. Under `RAIL_DISABLED=true` it allows every call.

`RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB` (default 16, from 1 to 100) bounds each request; an answer may be up to 1 KiB larger, since it echoes the body.

It serves `grpc.health.v1` on the same port: `""` is liveness, SERVING while it serves; `datrail.gateway.Ready` is readiness, SERVING once a bundle is held or with Rail Center disabled. Both are NOT_SERVING once it's stopping. `rail-apigee probe [--ready]` asks it, for an exec probe, and exits 0 only on SERVING.

On SIGTERM or Ctrl-C it drains for at most 5 s, then cuts the connections still open; with Rail Center on, its thread then has at most 1 s more to stop.

## The image

`docker build --target railguard-apigee .` builds it: distroless, as uid 10001, serving on 8080. `RAIL_CENTER_URL` and `RAIL_GATEWAY_SLUG` are required unless `RAIL_DISABLED=true`; An `http://` `RAIL_CENTER_URL` is refused unless `RAIL_ALLOW_HTTP=true`. `RAIL_AUTH_MODE=bearer` with `RAIL_AUTH_TOKEN`, an `rc_svc_` service token, sends Rail Center a credential. For a health check, run `/rail-apigee probe`, or `/rail-apigee probe --ready` for readiness.

## The Apigee proxy

`e2e/apigee/apiproxy/` is a proxy that does what rail-apigee needs; the e2e stack runs it on a stand-in for Apigee. Its `__…__` placeholders are the deployment's. What it must do, and what's left to the deployment:

- **`EC-Rail`**, an ExternalCallout policy in the request PreFlow, sending the request's headers and content (`with.request.headers` and `with.request.content`), and nothing of the response. Request streaming stays off: a streamed request reaches the callout with no content.
- **`TimeoutMs` well above 5000**, rail-apigee's deadline for a call, which starts once the request has arrived, so its answer arrives first; the stand-in uses 6000.
- **`RF-Refuse`** when `rail.decision` is `refuse`: a RaiseFault answering `rail.status` with `rail.body`.
- **Failing closed:** a FaultRule on `externalcallout.EC-Rail.failed`, and a step on `rail.decision = null`, both raising `RF-CalloutFailed`'s 503, so a callout that's down, times out, or answers without a decision forwards nothing. The FaultRule names EC-Rail, so another policy's fault keeps its own error.
- **The 4 MiB answer:** Apigee takes a callout answer of at most 4 MiB, and the answer echoes the body, so a request near or over 4 MiB fails closed whatever `RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB` allows.
- **The callout's own credential:** `EC-Rail` sends a Google ID token for a callout on Cloud Run; remove its `<Authentication>` otherwise. The target has none, so the agent's `Authorization` reaches the MCP server as sent.
