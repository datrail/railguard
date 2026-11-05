# The end-to-end stacks

Each stack runs a host's image behind its real proxy, with stubbed neighbours, and a driver that asserts what crossed the wire. The result is the driver's exit code.

```sh
make e2e-envoy-images   # build rail-envoy's image and pull the others; e2e-envoy builds them too
make e2e-envoy          # rail-envoy behind Envoy, per ticket state and host, over ext_authz and ext_proc, and their variants
make e2e-down           # remove a stack's containers and volumes; STACK=envoy by default
```

The envoy stack's TLS upstreams get a test CA and key made at each start (`envoy/certs.sh`), for that run only.

`envoy/envoy-variants.yaml` is one Envoy with a listener per config the examples don't show: both callouts in one chain, ext_authz's `encode_raw_headers` and body, and each ext_proc body mode. The driver expects the full-duplex body mode to fail (`xfail`): rail-envoy doesn't send that body back yet.
