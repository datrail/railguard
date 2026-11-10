# rail-envoy

The Envoy host: ext_authz and ext_proc over a unix socket (`RAIL_EXT_SOCKET`, default `/run/rail/ext.sock`). It allows every request.

For a request to a protected host, matched on its `:authority`, it sets:

- `x-rail` with this proxy's ticket from Rail Center, or `x-rail-status` with why there is none (neither under `RAIL_DISABLED=true`);
- `x-rail-upstream: tls`, or `plain` for an `http://` entry, for Envoy to route on;
- `:authority` to the host and port to connect to: the entry's port, else the agent's, where `:80` counts as none.

Any other request gets none of these. From every request it removes the agent's own `x-rail`, `x-rail-status` and `x-rail-upstream` but the one it sets, every other header whose name starts with `x-rail-`, and every header whose name has an `_`.

It serves `grpc.health.v1` on the same socket: SERVING while it serves, NOT_SERVING once it's stopping. `rail-envoy probe` (`rail probe` in its image) asks it, for an exec probe, and exits 0 only on SERVING; it waits at most 5 s, so give the probe a longer timeout. The ticket never affects health: without one, it fails closed by design, and a restart wouldn't help.

At `RAIL_LOG_LEVEL=debug` it logs each request's decision: the host, and for a protected one the header set (`x-rail`, or `x-rail-status` with its reason), the route and where it connects; and the headers removed. Never the ticket.

On SIGTERM or Ctrl-C it reports NOT_SERVING and drains for at most 5 s, then cuts the connections still open: Envoy keeps them, and an ext_proc stream lasts as long as its request. With Rail Center on, its thread then has at most 1 s more to stop, so 6 s in all.

`RAIL_PROXY_PROTECTED_HOSTS` lists the protected hosts, comma-separated, each `[http(s)://]host[:port]`: HTTPS unless it says `http://`, and its port if it names one. It's required unless `RAIL_DISABLED=true`. Anything else, and a host listed twice, is refused.

Envoy's configuration is the customer's, so we can only recommend it. rail-envoy needs at least:

- ext_authz or ext_proc on the request headers, over the unix socket;
- either fail closed (`failure_mode_allow: false`), or fail open with Envoy removing `x-rail`, `x-rail-*` and `_` headers before the route is picked and before the callout: `early_header_mutation_extensions` and `headers_with_underscores_action: DROP_HEADER` on the connection manager. Otherwise, while rail-envoy is down, the agent's own headers cross, and its `x-rail-upstream` picks the route. Failing open, while it's down a protected host's requests go as the agent sent them, over plain HTTP, usually at port 80, so they usually fail to connect, and no ticket is sent;
- every request header reported: no `allowed_headers` or `disallowed_headers` on ext_authz, no `forward_rules` on ext_proc. Otherwise only `x-rail`, `x-rail-status` and `x-rail-upstream` are sure to be removed;
- `allow_all_routing` (ext_authz's `decoder_header_mutation_rules`, ext_proc's `mutation_rules`), so it can rewrite `:authority`, and no rules that refuse its other changes, which Envoy may otherwise drop silently;
- the route picked again after the callout: `clear_route_cache: true` on ext_authz; ext_proc's answer asks for it;
- routes on `x-rail-upstream` to an HTTPS and a plain dynamic forward proxy cluster, each removing the header before the upstream.

[`e2e/envoy/envoy-ext-authz.yaml`](../../e2e/envoy/envoy-ext-authz.yaml) and [`envoy-ext-proc.yaml`](../../e2e/envoy/envoy-ext-proc.yaml) are working examples; they fail open.
