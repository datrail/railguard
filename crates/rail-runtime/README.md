# rail-runtime

The IO around `rail-policy`: typed configuration from the environment, the one client every Rail Center call goes through, and the holders that keep the ticket and the policy bundle through Rail Center failures.

It also holds the gateway role a host calls: `Gateway::handle` judges each request against the bundle held, by its deadline: forward it, deny it (403), or refuse it as one the rules can't apply to (503).
