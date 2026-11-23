# rail-model

DatRail's rules language. Rail Center publishes the rules as a policy bundle: which calls to which MCP tools are
allowed, judged on what an agent's ticket says about it. This crate reads bundles, tickets and capture plans, reads
which endpoint and host a call names, judges the call against the rules, and reads a call's request line, redacted, for
capture. See the [main README](../../README.md) for the system around it.

It's one crate, used everywhere a rule is read, so a rule means the same thing where it's written and where it's
enforced. The conformance cases in [`vectors/`](../../vectors) define its behaviour.

## Who uses it

Rail Center and RailDash, outside this workspace, are to use it through the Python package
[`rail-model-py`](../rail-model-py/README.md): Rail Center to check each rule before publishing it, and RailDash to try
a rule on a sample call.

## Entry points

By module:

- **`policy`:** **`parse_bundle`** reads a policy bundle, refusing one it can't use with why; **`compile_bundle`** makes
  it a `CompiledBundle`, ready to judge with. **`judge`** judges one call against a `CompiledBundle`; **`judge_traced`**
  also hands a `Tracer` each policy evaluated. **`parse_ticket`** reads an `x-rail` value: base64, or a signed JWT
  checked against a `SignedTicketConfig`'s keys, issuer and audience. **`parse_key_set`** reads Rail Center's signing
  keys.
- **`condition`:** **`check_condition`** reports every problem with one condition, and whether every reader refuses it
  or only Rail Center; **`compile_condition`** and **`evaluate_condition`** evaluate one, as the judge does. A
  **`Path`** is spelled as a dotted string or as a list, in a bundle or a plan; it's read for a **`ConditionUse`** (a
  policy, a plan's match, a plan's context key or measure), which decides the paths it reads; a condition is compiled
  for one and evaluated in the **`Environment`** that use fills (`for_judging`, or `for_capture`, from a
  **`RequestLine`** and the body). A body value that can't be told, or isn't read, is a verdict of its own (`Ambiguous`,
  `BodyUnread`), never false; a refusal keeps its cause.
- **`endpoint`:** **`Endpoint`** and **`ResolvedKey`**, the endpoint a call asks for, keyed as a bundle binds it, and
  **`Resolution`**, whether a message is judged at all.
- **`agent`:** **`AgentId`**, which agent a host serves, as Rail Center names it: its tickets are fetched for it, and a
  capture plan names it.
- **`authority`:** **`split_authority`** reads a host and port from an `:authority`, spelled one way wherever a host is
  compared.
- **`body`:** **`BodyBuffer`** keeps a body as it arrives, up to a cap and within a **`ByteBudget`** every kept body
  shares, as a **`RequestBody`**: read once into a JSON tree (**`json`**, walked with `tree::Node`, a walk finding a
  value, nothing, or one that can't be told) and an MCP reading (**`mcp`**, or why it names nothing). **`BodyRead`** is
  a call's body as a reader has it: kept whole, or unread and why. **`mcp::read_endpoint`** reads the endpoint an MCP
  message names, from its method and tool; **`McpReading::read`** reads one from bytes a host holds itself.
- **`capture`:** **`parse_plan`** reads a capture plan as `parse_bundle` reads a bundle, passing over fields it doesn't
  know, refusing one that breaks the field reference with why (**`InvalidPlan`**). A **`Plan`**'s entries keep their
  conditions as written, checked when compiled; its values are as Rail Center writes them. It checks the proxy's
  ceilings too. Its `content_hash`'s form, its `schema_version`'s major, its agent and its `redaction_version` are the
  caller's to check. **`compile_plan`** compiles a read plan's entries for matching calls (**`CompiledPlan`**): an entry
  whose condition can't be evaluated for a plan, or whose conditions don't pin one endpoint, is refused with why
  (**`EntryRefusal`**), and a call reaching it goes to the catch-all; entries are indexed by the host they name, as
  written, and a refused entry naming none stops the walk for every host.
- **`schema_version`:** **`SchemaVersion`**, a document's `MAJOR.MINOR`, a bundle's or a plan's.
- **`request_line`:** **`RequestLine::read`** reads a call's `:authority`, `:method` and `:path` once, as conditions
  read them under `request`: the host as `split_authority` spells it, or `[unparseable]`; the path up to its query or
  fragment (`/` for a `CONNECT`), as received and redacted. It also tells why a path can match no capture plan entry
  (**`unmatchable`**), and what a call record reports of the call (**`reported_method`**, **`path_prefix`**).
- **`redaction`:** RailMon's rule for a path, at **`REDACTION_VERSION`**, as Rail Center writes a capture plan's paths.
- **`sanitize`:** **`sanitize`**, **`sanitize_quoted`**, **`sanitize_unquoted`** and **`sanitize_for_json`** make text
  from a call safe to log.

## What it doesn't do

- **No I/O:** no network, no files, no clock (the time is passed in), no log. So it behaves the same in a gateway, in
  Rail Center and in a test, and its callers decide what to fetch and when.
- **No settings:** what a host is configured with is `rail-runtime`'s.
