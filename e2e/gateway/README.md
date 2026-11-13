# The gateway's shared cases

What the stacks testing a gateway share: the apigee stack, through the Apigee stand-in, and the envoy-gateway stack,
through each Envoy listener. See [the stacks](../README.md).

- `gateway_cases.py`: the cases every host's gateway passes alike, through an MCP URL (a call without a ticket refused,
  a good one forwarded with only the agent's headers, a ticket sent twice, a claimed status, the query kept out of the
  endpoint key, a body that isn't UTF-8), and what the drivers share: the reporting, the stubs' journals, and MCP calls.
  Each driver mounts it beside itself.
- `rail-center-mappings/`: the bundle (to the service token only), the signing keys, and the denials route.
- `upstream-mappings/`: the MCP server.
- `stub.Dockerfile`: either stub with its mappings baked in, for the live harnesses on Cloud Run.
- `live.sh`: what both live harnesses' `session.sh` share: the image builds, the wait for the upstream, the stubs' admin
  check.
