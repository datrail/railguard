# The end-to-end stacks

Tests that run each host's real image in front of the software it plugs into, Envoy or Apigee, and check what actually crossed the wire. Unit tests show a host does what we meant; these show Envoy and Apigee take what it answers as we meant, with their configuration as we recommend it.

Each stack runs a host's image behind its real proxy, with stubbed neighbours, and a driver that asserts what crossed the wire. The result is the driver's exit code.

| Suite | What it tests |
|---|---|
| [`envoy`](envoy/README.md) | rail-envoy behind Envoy, per ticket state and host, over ext_authz and ext_proc, and their variants |
| [`apigee`](apigee/README.md) | rail-apigee on its own, then behind a stand-in for Apigee running `apigee/apiproxy` |
| [`apigee/live`](apigee/live/README.md) | the apigee suite's driver against real Apigee X, with rail-apigee on Cloud Run: paid, by hand, never in CI |

```sh
make e2e-envoy-images   # build rail-envoy's image and pull the others; e2e-envoy builds them too
make e2e-envoy          # rail-envoy behind Envoy, per ticket state and host, over ext_authz and ext_proc, and their variants
make e2e-apigee-images  # build rail-apigee's image and the stand-in's, and pull the others
make e2e-apigee         # rail-apigee on its own, then behind a stand-in for Apigee running apigee/apiproxy
make e2e-down           # remove a stack's containers and volumes; STACK=envoy by default
```
