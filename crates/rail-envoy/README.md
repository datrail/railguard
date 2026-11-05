# rail-envoy

The Envoy host: ext_authz and ext_proc over a unix socket (`RAIL_EXT_SOCKET`, default `/run/rail/ext.sock`). It allows every request, sets `x-rail` with this proxy's ticket from Rail Center, or `x-rail-status` with why there is none (neither under `RAIL_DISABLED=true`), and removes the agent's own `x-rail` header, every header whose name starts with `x-rail-` (so not `x-railing`), and every header whose name has an `_`.

Envoy's configuration is the customer's, so we can only recommend it. rail-envoy needs at least:

- ext_authz or ext_proc on the request headers, over the unix socket;
- fail closed: `failure_mode_allow: false`;
- every request header reported: no `allowed_headers` or `disallowed_headers` on ext_authz, no `forward_rules` on ext_proc. Otherwise only `x-rail` and `x-rail-status` are sure to be removed;
- no header mutation rules that refuse its changes (`decoder_header_mutation_rules` on ext_authz, `mutation_rules` on ext_proc), which Envoy may otherwise drop silently.

[`e2e/envoy/envoy-ext-authz.yaml`](../../e2e/envoy/envoy-ext-authz.yaml) and [`envoy-ext-proc.yaml`](../../e2e/envoy/envoy-ext-proc.yaml) are working examples.
