# The end-to-end stacks

Each stack runs a host's image behind its real proxy, with stubbed neighbours, and a driver that asserts what crossed the wire. The result is the driver's exit code.

```sh
make e2e-envoy-images   # build rail-envoy's image and pull the others; e2e-envoy builds them too
make e2e-envoy          # rail-envoy behind Envoy, per ticket state and host, over ext_authz and ext_proc, and their variants
make e2e-apigee-images  # build rail-apigee's image and the stand-in's, and pull the others
make e2e-apigee         # rail-apigee on its own, then behind a stand-in for Apigee running apigee/apiproxy
make e2e-down           # remove a stack's containers and volumes; STACK=envoy by default
```

The envoy stack's TLS upstreams get a test CA and key made at each start (`envoy/certs.sh`), for that run only.

`envoy/envoy-variants.yaml` is one Envoy with a listener per config the examples don't show: both callouts in one chain, ext_authz's `encode_raw_headers` and body, and each ext_proc body mode, each failing closed. Each body case expects the body to reach the upstream whole, with the agent's trailers or past ext_authz's `max_request_bytes` too, and the agent to get the upstream's answer. The driver expects the full-duplex body mode to fail (`xfail`), and only as known, a 500 with nothing received: rail-envoy doesn't send that body back yet.

The apigee stack has no real Apigee: `apigee/apigee_standin.py` models a proxy running `apigee/apiproxy/`, each rule naming its source, and calls rail-apigee as Apigee would. It and the driver run on Python with gRPC (`apigee/python.Dockerfile`), with the ExternalCallout stubs generated from the vendored `.proto` and the packages pinned by hash (`apigee/requirements.txt`).

`apigee/live/` runs the apigee stack's driver against real Apigee X, with rail-apigee on Cloud Run: paid, by hand, never in CI (`apigee/live/README.md`).
