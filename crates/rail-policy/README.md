# rail-policy

DatRail's rules language. Rail Center publishes the rules as a policy bundle: which calls to which MCP tools are
allowed, judged on what an agent's ticket says about it. This crate reads bundles and tickets, and judges one call
against the rules. See the [main README](../../README.md) for the system around it.

It's one crate, used everywhere a rule is read, so a rule means the same thing where it's written and where it's
enforced. The conformance cases in [`vectors/`](../../vectors) define its behaviour.

## Who uses it

- **The gateways,** through `rail-runtime`, to judge each call.
- **Rail Center and RailDash,** through the Python package [`rail-policy-py`](../rail-policy-py/README.md), to check a
  rule before it's published and to try one on a sample call.

## Entry points

- **`parse_bundle`** reads a policy bundle, refusing one it can't use with why; **`compile_bundle`** makes it a
  `CompiledBundle`, ready to judge with.
- **`parse_ticket`** reads an `x-rail` value: base64, or a signed JWT checked against a `SignedTicketConfig`'s keys,
  issuer and audience. **`parse_key_set`** reads Rail Center's signing keys.
- **`resolve_endpoint`** names the endpoint a call asks for, from its path, method and tool.
- **`judge`** judges one call against a `CompiledBundle`; **`judge_traced`** also hands a `Tracer` each policy
  evaluated.
- **`check_condition`** reports every problem with one condition, and whether every reader refuses it or only Rail
  Center.
- **`sanitize`**, **`sanitize_quoted`** and **`escape_unquoted`** make text from a call safe to log.

## What it doesn't do

- **No I/O:** no network, no files, no clock (the time is passed in), no log. So it behaves the same in a gateway, in
  Rail Center and in a test, and its callers decide what to fetch and when.
- **No settings:** what a host is configured with is `rail-runtime`'s.
