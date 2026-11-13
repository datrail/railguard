# The end-to-end stacks

Tests that run each host's real image behind Envoy, or a stand-in for Apigee, with stubbed neighbours, and a driver that
asserts what actually crossed the wire; the result is its exit code. Unit tests show a host does what we meant; these
show Envoy and Apigee take what it answers as we meant, with their configuration as we recommend it. See the
[main README](../README.md) for the system they test.

## What a stack is

A Docker Compose project: one host's image, built from this checkout as it ships; the software it plugs into; WireMock
stubs for Rail Center and for the MCP servers; and a driver, which sends calls through it and reads back from each
stub's request journal what arrived. The stack exits with the driver.

| Suite                                      | What it tests                                                                                               |
| ------------------------------------------ | ----------------------------------------------------------------------------------------------------------- |
| [`envoy-proxy`](envoy-proxy/README.md)     | rail-envoy's proxy behind Envoy, per ticket state and host, over ext_authz and ext_proc, and their variants |
| [`envoy-gateway`](envoy-gateway/README.md) | rail-envoy's gateway behind Envoy in GCP's load balancer's place, over each way Envoy calls it              |
| [`apigee`](apigee/README.md)               | rail-apigee on its own, then behind a stand-in for Apigee running `apigee/apiproxy`                         |
| [`apigee/live`](apigee/live/README.md)     | the apigee suite's driver against real Apigee X, with rail-apigee on Cloud Run: paid, by hand, never in CI  |

## Commands

```sh
make e2e-envoy-proxy-images    # build rail-envoy's image and pull the others; e2e-envoy-proxy builds them too
make e2e-envoy-proxy           # the envoy-proxy suite
make e2e-envoy-gateway-images  # the same for the envoy-gateway suite
make e2e-envoy-gateway         # the envoy-gateway suite
make e2e-apigee-images  # build rail-apigee's image and the stand-in's, and pull the others
make e2e-apigee         # the apigee suite
make e2e-down           # remove a stack's containers and volumes; STACK=envoy-proxy by default
```

They need Docker with Compose v2. CI runs the envoy-proxy, envoy-gateway and apigee suites on each change, one job per
stack, and prints the stack's logs when one fails.

## What the suites share

- **What arrived, not what was sent:** each driver asserts on the stubs' journals, so a header a host meant to remove,
  but that Envoy or Apigee kept, fails the run.
- **Every image is pinned by digest,** so a run tests what the last one did.
- **Tickets are unsigned:** no case sends a signed one yet.
