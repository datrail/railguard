# rail-runtime

What a running host does around the rules language: it reads the host's settings, talks to Rail Center, and keeps what
it fetched usable while Rail Center can't be reached. Both hosts, the gateway and the proxy, are built on it, so they
behave the same; it knows nothing of Envoy, Apigee or gRPC, which each host's own crate adds.

The IO around `rail-policy`: typed configuration from the environment, the one client every Rail Center call goes
through, and the holders that keep the ticket and the policy bundle through Rail Center failures.

It also holds the gateway role a host calls: `Gateway::handle` judges each request against the bundle held, by its
deadline: forward it, deny it (403), or refuse it as one the rules can't apply to (503).

Both hosts log to stderr through `logging`. The settings it reads for both, and for the gateway's bundle and the proxy's
ticket, are in [`CONFIGURATION.md`](../../CONFIGURATION.md).
