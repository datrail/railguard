# The envoy stack

Checks that rail-envoy, behind a real Envoy, attaches tickets as it should: for each state a ticket can be in (held, not found, expired, Rail Center down, disabled), and for each kind of protected host, the right headers reach the right upstream, and nothing reaches the wrong one. It also checks the configurations rail-envoy's README recommends, and that Envoy fails open as described when rail-envoy is down.

`make e2e-envoy` runs it (see [the stacks](../README.md)). `compose.yml` holds:

- `rail-center`, a WireMock stub answering by sandbox name (`rail-center/mappings`);
- one rail-envoy per ticket state (`ext`, `ext-not-found`, `ext-expired`, `ext-issuer-down`, `ext-disabled`), each with an Envoy in front on `envoy-ext-authz.yaml`; and `envoy-ext-proc`, on `envoy-ext-proc.yaml`;
- `envoy-no-ext-authz` and `envoy-no-ext-proc`, whose socket nothing serves on, so every callout fails and Envoy fails open;
- `envoy-variants`, on `envoy-variants.yaml` (below);
- the upstreams, WireMock: plain HTTP, HTTPS on 8443, and HTTPS on 443 (`upstream-mappings`);
- `driver.py`, which sends requests through each Envoy and asserts what reached each upstream.

`envoy-ext-authz.yaml` and `envoy-ext-proc.yaml` are the working examples rail-envoy's README points to.

The envoy stack's TLS upstreams get a test CA and key made at each start (`envoy/certs.sh`), for that run only.

`envoy/envoy-variants.yaml` is one Envoy with a listener per config the examples don't show: both callouts in one chain, ext_authz's `encode_raw_headers` and body, and each ext_proc body mode, each failing closed. Each body case expects the body to reach the upstream whole, with the agent's trailers or past ext_authz's `max_request_bytes` too, and the agent to get the upstream's answer. The driver expects the full-duplex body mode to fail (`xfail`), and only as known, a 500 with nothing received: rail-envoy doesn't send that body back yet.
