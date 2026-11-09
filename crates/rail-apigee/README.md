# rail-apigee

The Apigee host: Apigee's ExternalCallout service over TCP on `RAIL_GATEWAY_PORT` (default 8080). It judges each `POST` against the policy bundle held, and answers with what Apigee should change: the received context, with every `x-rail` and `x-rail-*` header removed, and the flow variable `rail.decision` set to `allow` or `refuse`, plus `rail.status` and `rail.body` for a refusal. Nothing else in the context changes. Under `RAIL_DISABLED=true` it allows every call.

`RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB` (default 16, from 1 to 16) bounds each request; an answer may be up to 1 KiB larger, since it echoes the body.

It serves `grpc.health.v1` on the same port: `""` is liveness, SERVING while it serves; `datrail.gateway.Ready` is readiness, SERVING once a bundle is held or with Rail Center disabled. Both are NOT_SERVING once it's stopping. `rail-apigee probe [--ready]` asks it, for an exec probe, and exits 0 only on SERVING.

On SIGTERM or Ctrl-C it drains for at most 5 s, then cuts the connections still open; with Rail Center on, its thread then has at most 1 s more to stop.
