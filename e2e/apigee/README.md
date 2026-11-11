# The apigee stack

Checks rail-apigee as Apigee would use it, without paying for Apigee: first rail-apigee on its own (health, readiness, a call judged), then behind a stand-in that does what an Apigee proxy running `apiproxy/` does with its answers. The cases include what the Python gateway's runs on real Apigee found Apigee doing, such as splitting a header on its commas, so the stand-in is held to it. [`live/`](live/README.md) runs the same driver against real Apigee, to check the stand-in is right.

`make e2e-apigee` runs it (see [the stacks](../README.md)). `compose.yml` holds:

- `rail-center`, a WireMock stub serving the bundle (to the service token only), the signing keys, and taking denials (`rail-center-mappings`);
- `upstream`, a WireMock MCP server (`upstream-mappings`);
- `callout`, rail-apigee enrolled with the stub; and `callout-unreachable`, on port 9100 with a Rail Center it can't reach, which must still start;
- `apigee`, the stand-in, with two proxies: `/mcp`, judged by `callout`, and `/no-callout`, whose callout never resolves;
- `driver.py`, which asserts what reached each stub, from their request journals.

`apiproxy/` is the Apigee proxy rail-apigee's README describes, with its placeholders. `RF-E2E-OperatorFault.xml` is a test-only policy, faulting before the callout, to check an operator's own fault keeps its error.

The apigee stack has no real Apigee: `apigee/apigee_standin.py` models a proxy running `apigee/apiproxy/`, each rule naming its source, and calls rail-apigee as Apigee would. It and the driver run on Python with gRPC (`apigee/python.Dockerfile`), with the ExternalCallout stubs generated from the vendored `.proto` and the packages pinned by hash (`apigee/requirements.txt`).
