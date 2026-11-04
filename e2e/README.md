# The end-to-end stacks

Each stack runs a host's image behind its real proxy, with stubbed neighbours, and a driver that asserts what crossed the wire. The result is the driver's exit code.

```sh
make e2e-envoy   # rail-envoy behind Envoy, over ext_authz and over ext_proc
make e2e-down    # remove the stack's containers and volumes
```

Carried over from `datrail/proxy`'s `e2e/envoy-grpc`, a case at a time. For now: a request crosses under each callout, and nothing crosses when the extension is down.
