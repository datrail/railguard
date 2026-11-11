# RailGuard

DatRail lets an organisation decide what its AI agents may do with the MCP servers they call. Rail Center, a separate service, keeps the rules, and issues each agent a ticket saying what it knows about that agent. RailGuard enforces those rules on the network: a gateway in front of MCP servers judges each call against the rules, and a proxy beside the agents attaches their tickets to the calls they make.

It's one Rust workspace for both, and for the rules language they share. The language is also built as a Python package, so Rail Center can check a rule with the same code the gateway enforces it with. RailGuard is to replace the Python repos `datrail/gateway` and `datrail/proxy`, one image at a time.

## What's here

| Path | What it is |
|---|---|
| [`crates/rail-policy`](crates/rail-policy/README.md) | The rules language: reads policy bundles and tickets, and judges a call. |
| [`crates/rail-policy-py`](crates/rail-policy-py/README.md) | The language as a Python package, `datrail-policy`, for Rail Center and RailDash. |
| [`crates/rail-runtime`](crates/rail-runtime/README.md) | What a running host does around the language: its settings, Rail Center, and the gateway's judging of each call. |
| [`crates/rail-grpc-host`](crates/rail-grpc-host/README.md) | The process parts both gRPC hosts share: health checks and shutdown. |
| [`crates/rail-apigee`](crates/rail-apigee/README.md) | The gateway, as an Apigee callout. |
| [`crates/apigee-callout-proto`](crates/apigee-callout-proto/README.md) | Apigee's callout interface, as Rust code. |
| [`crates/rail-envoy`](crates/rail-envoy/README.md) | The proxy, as an Envoy extension. |
| [`crates/rail-test-support`](crates/rail-test-support/README.md) | What the tests share. |
| [`e2e`](e2e/README.md) | End-to-end tests: each host's image in front of the software it plugs into. |
| `vectors` | The conformance cases that define how the language and the gateway behave. |

## Building and testing

```sh
make lint
make test
make test-py
```

`make check` runs all three, as CI does. `crates/rail-policy-py` builds the `datrail-policy` wheel. `make test-py` needs [uv](https://docs.astral.sh/uv/). The end-to-end stacks are run separately; see [`e2e`](e2e/README.md).
