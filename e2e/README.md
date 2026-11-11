# The end-to-end stacks

Tests that run each host's real image behind Envoy, or a stand-in for Apigee, with stubbed neighbours, and a driver that asserts what actually crossed the wire; the result is its exit code. Unit tests show a host does what we meant; these show Envoy and Apigee take what it answers as we meant, with their configuration as we recommend it.

| Suite | What it tests |
|---|---|
| [`envoy`](envoy/README.md) | rail-envoy behind Envoy, per ticket state and host, over ext_authz and ext_proc, and their variants |
| [`apigee`](apigee/README.md) | rail-apigee on its own, then behind a stand-in for Apigee running `apigee/apiproxy` |
| [`apigee/live`](apigee/live/README.md) | the apigee suite's driver against real Apigee X, with rail-apigee on Cloud Run: paid, by hand, never in CI |

```sh
make e2e-envoy-images   # build rail-envoy's image and pull the others; e2e-envoy builds them too
make e2e-envoy          # the envoy suite
make e2e-apigee-images  # build rail-apigee's image and the stand-in's, and pull the others
make e2e-apigee         # the apigee suite
make e2e-down           # remove a stack's containers and volumes; STACK=envoy by default
```
