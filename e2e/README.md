# The end-to-end stacks

Each stack runs a host's image behind its real proxy, with stubbed neighbours, and a driver that asserts what crossed the wire. The result is the driver's exit code.

```sh
make e2e-envoy-images   # build rail-envoy's image and pull the others; e2e-envoy builds them too
make e2e-envoy          # rail-envoy behind Envoy, per ticket state and host, over ext_authz and ext_proc
make e2e-down           # remove a stack's containers and volumes; STACK=envoy by default
```

The envoy stack's TLS upstream gets a test CA and key made at each start (`envoy/certs.sh`), for that run only.
