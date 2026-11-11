# rail-runtime

The IO around `rail-policy`: typed configuration from the environment, the one client every Rail Center call goes through, and the holders that keep the ticket and the policy bundle through Rail Center failures.

It also holds the gateway role a host calls: `Gateway::handle` judges each request against the bundle held, by its deadline: forward it, deny it (403), or refuse it as one the rules can't apply to (503).

Both hosts log to stderr through `logging`. `RAIL_LOG_LEVEL` (`error`, `warn`, `info`, `debug` or `trace`; `info` by default) sets the level of this workspace's own lines; the libraries' stay at `warn`, since below it they log the wire itself, reqwest's and rustls' included, which log through `log`.
