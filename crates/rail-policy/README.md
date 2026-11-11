# rail-policy

DatRail's rules language. Rail Center publishes the rules as a policy bundle: which calls to which MCP tools are
allowed, judged on what an agent's ticket says about it. This crate reads bundles and tickets, and judges one call
against the rules.

It's one crate, used everywhere a rule is read: the gateways judge with it, and Rail Center and RailDash are to check
rules with it through the Python package (`rail-policy-py`). So a rule means the same thing where it's written and where
it's enforced. It does no I/O, which keeps it the same in all of them; the conformance cases in `vectors/` define its
behaviour.

It reads the policy bundle Rail Center publishes and the `x-rail` tickets agents present, base64 or signed JWT.
