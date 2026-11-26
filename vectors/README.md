# Conformance vectors

The behaviours of `rail-model`, the reader of the policy bundle and the capture plan, written as data: each case an
input and what this reader answers. They're for two things:

- **Other implementations,** checked by the same cases: Rail Center's Python, through the `datrail-model` wheel, and any
  later reader.
- **Reviewing a behaviour as data:** a case is named by its `id`, so a doc or a proposal can point at it, and a change
  to a behaviour shows as the `expect`s it changes.

Run them with `cargo test -p rail-model -- vectors::` (`make coverage-vectors` for what they alone cover).
`make test-py` runs the `policy/` files, layer 4's conditions and layer 3's judging paths through the wheel's Python
interface. `gateway_enforcement_conformance_cases.json` is `rail-runtime`'s, of the gateway role, in a format of its
own.

## The format

Every file holds `cases`, each:

- **`id`:** a kebab-case slug saying what the case checks, unique across the files; never renamed, so a case whose
  meaning changes gets a new one.
- **`note`,** where the `id` can't say all: why the answer is what it is, or an input spelled out that the file can't
  show (bytes that aren't UTF-8).
- **its inputs, then `expect`,** the whole of what this reader answers: a case states this implementation's answer only;
  where another answers otherwise is the contract's to say. The ported files below keep their sources' fields: their
  answers in named fields, as the layers table says, not under `expect`.

A file may also hold the inputs its cases share. In a ported file, `added` marks a case that is this reader's own.

**Whose a behaviour is** follows from where it lives: `bundle`, the policy bundle's language, shared by the gateway's
policies and the capture plan's matches (a change reaches the gateway's enforcement); or `capture`, capture's own (a
change reaches only the call records).

| Where                                                                                            | Whose     |
| ------------------------------------------------------------------------------------------------ | --------- |
| `request_line`, `redaction`, `capture/*`                                                         | `capture` |
| `mcp`, which the gateway resolves a call's endpoint by too                                       | `bundle`  |
| `condition/condition`, `condition/op`, `policy/*`                                                | `bundle`  |
| `condition/path`: `request`, and paths under `ticket`, `request.headers`, `request.endpoint_key` | `bundle`  |
| `condition/path`: `request.host`, `method`, `path`, `redacted_path`, `query`, `mcp`, `body`      | `capture` |

## The layers

A call is read first (its request line, its MCP message); then a condition and a record share a path's resolution. A
condition's op tests the value, and an entry's "and" joins its conditions' verdicts; a record's keys and measures read
the call, and a window writes them.

| File                        | Layer                                                        | A case's inputs                                                                                           | `expect`                                                                                                                                           |
| --------------------------- | ------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| `request_line.json`         | 1. A call's request line                                     | `authority`, `method`, `path` as sent                                                                     | `host`, `method` as read, `reported_method`, `path`, `redacted_path`, `segments`, `redacted_segments`, `query` (each name's values), `unmatchable` |
| `mcp.json`                  | 2. What a body names as an MCP message                       | `body`; `path`, the upstream's                                                                            | `mcp`, the reading; `endpoint`, what it and the path resolve to                                                                                    |
| `condition/path.json`       | 3. A path, read for a use, resolved                          | `environment`; `use`: `gateway_policy`, `capture_plan` or `record`; `path`, either spelling; `step_limit` | the value, or a refusal                                                                                                                            |
| `condition/condition.json`  | 4. A condition read, compiled for a use, checked             | `condition` as sent; `use`; the file's `ticket_claims_schema`                                             | `reading`; `compiled`: `evaluable` or a refusal; `problems`, Rail Center's check's, each without its wording                                       |
| `condition/op.json`         | 5. An op on a value                                          | `value`; `op`; `operand`, none for no operand; `step_limit`                                               | a verdict                                                                                                                                          |
| `capture/entry.json`        | 6. An entry's "and"                                          | `environment`; `match`, an entry's conditions; `step_limit`                                               | a verdict                                                                                                                                          |
| `capture/record.json`       | 7. What a record reads of a call                             | `environment`; `record_of`: `entry` or `catch_all`; `record`, as a plan writes it; `step_limit`           | `context_keys` and `measures`, each reading by its name                                                                                            |
| `capture/matching.json`     | 8. A plan compiled, and the record taking a call             | `plan`; `entries`; `call`                                                                                 | `refused` entries, `entries_skipped`, `taken`                                                                                                      |
| `capture/window.json`       | 9. A window's calls, closed into its document                | `plan`; `entries`; `start`, `end`; `calls`, each at a time; `calls_dropped`; `sent`, the sender's values  | the whole document                                                                                                                                 |
| `capture/windowing.json`    | 10. Windowing over chosen times                              | `plans`, those its events name; `events`                                                                  | each event's output                                                                                                                                |
| `authority.json`            | An authority read                                            | `authority`                                                                                               | `host`, `port`, `null` for none                                                                                                                    |
| `redaction.json`            | A path redacted                                              | `path`                                                                                                    | `redacted`                                                                                                                                         |
| `policy/bundle.json`        | A bundle read                                                | `bundle`                                                                                                  | `usable`; a usable one's `chain`, its enabled policies in evaluation order, and their `chain_priorities`                                           |
| `policy/judge.json`         | A call judged against a bundle: the fallbacks, then the walk | `bundle`, `mode`, `endpoint`, `ticket`, `headers`                                                         | `expect.fallbacks`, those that deny, in the order asked; `expect.chain`, the walk's verdict                                                        |
| `policy/base64_ticket.json` | An unsigned ticket read                                      | `header`, `now_in_seconds`                                                                                | `state`; a valid one's `token` and `exp_in_seconds`                                                                                                |
| `policy/jwt_ticket.json`    | A signed ticket read                                         | `header`, `now_in_seconds`; `reader`, in place of the file's                                              | `state`; a valid one's `token` and `exp_in_seconds`                                                                                                |
| `capture/plan.json`         | A capture plan read                                          | `patch` on the file's `plan`, none for the plan as written                                                | `valid`; `conditions`, the first entry's as read: `valid`, `unknown` or `invalid`, each checked when compiled                                      |

Times are Unix seconds. A value's whole number below 2^53 is written as an integer (`1`, never `1.0`), from 2^53 as a
double; every number is compared as written. An object's keys are compared in any order, so a document's field order is
a Rust test's.

### Shared inputs

- **`environments.json`:** named environments. One `for` judging, a policy's sample as `for_judging` builds it (the
  gateway builds its own from the request, a policy reading it alike but for `host`, taken from `:authority`): the
  ticket's claims (`null` for no usable ticket, so every ticket path is absent), the `endpoint` (`keyless`,
  `unrecognised` or `{"resolved": key}`) and the `headers` as sent; its request line reads absent, its body as not sent.
  One `for` capture: a call, with no ticket and no header lines, its endpoint the one its path and body name; a body
  unread reads its endpoint key as unread.
- **A call:** its `authority`, `method` and `path` as sent; its `body` as text, as `{"hex": …}` for bytes that aren't
  UTF-8, as `{"unread": why}` (`past_cap`, `past_budget`, `not_sent`, `cut_short`), or as `{"text": …, "bytes": n}`,
  where `<N bytes>` stands for N bytes of `x` and the text is padded with spaces to `bytes`; `body_bytes`, its length as
  the call declares it; `byte_budget`, where a case is about passing the bytes a body and its tree share (where none is
  given, a finite one past any case's body).
- **`capture/plans.json`:** named plans. A case's `plan` is a name, its `entries`, where given, in place of the plan's;
  or a plan written whole. `one-entry` is the plan layers 6 and 7 write their `match` or `record` into; `matching`,
  layer 8's, two MCP tools on one host and an HTTP operation on another, their records without measures;
  `worked-example` is the capture design's (railxia/docs' `design/2026-11-10-rc-dg-capture-plan/design.md` at `95b5c7b`,
  its section 2) with its measures as this reader reads them, `two-groups` it with a smaller budget, and `sixty-seconds`
  and `five-minutes` it with another window and their own `plan_version`s, 1 and 2, by which layer 10 tells their
  windows apart.

### Values

Every value is tagged, never bare. A variant holding nothing is its snake-case name, one holding something a one-key
object of it:

| Type                     | Encoded                                                                                                                                                                                                                      |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A path's value           | `"absent"`, `{"string": "…"}`, `{"number": 1.5}` (`"inf"`, `"-inf"` past a double), `{"bool": true}`, `{"list": [<value>, …]}`, `"object"`, `"ambiguous"`, `{"body_unread": why}`                                            |
| Why a body isn't read    | `{"unread": "past_cap" \| "past_budget" \| "not_sent" \| "cut_short"}`, `{"unparsed": "malformed" \| "past_budget"}`, `"batch"`                                                                                              |
| A verdict                | `"holds"`, `"does_not_hold"`, `"ambiguous"`, `{"body_unread": why}`, `{"refused": why}`                                                                                                                                      |
| A refusal                | `{"refused": "unknown_op" \| "invalid" \| "not_for_use" \| "budget_exceeded"}` as resolved or evaluated; layer 3's `{"refused_reading": "invalid" \| "not_for_use" \| "not_for_record"}` as read for its use, never resolved |
| A record's path          | `{"found": <kept>}`, `"absent"`, `"ambiguous"`, `{"body_unread": why}`, `{"refused": why}`                                                                                                                                   |
| What a key keeps         | `{"string": "…"}`, `{"hashed": "<64 hex digits>"}`, `{"replaced": "key_shape" \| "past_longest"}`, `"not_a_string"`; a size's `{"bytes": n}`, an `is_set`'s `{"set": true \| false}`                                         |
| A host                   | `{"read": host}`; `"refused"`, `"past_longest"`, each read and reported as `[unparseable]`                                                                                                                                   |
| A method reported        | the method, one of the standard nine, or `"other"`, reported as `[other]`                                                                                                                                                    |
| Why a path never matches | `null`, or `"not_utf8"`, `"bracket"`, `"encoded_slash"`, `"dot_segment"`, `"empty_segment"`, the first that applies in that order                                                                                            |
| An MCP reading           | `{"read": {"method": …, "tool": …}}`, each `null` where it names none; `{"unparsed": why}`, `"batch"`, `"ambiguous"`                                                                                                         |
| An endpoint              | `"discovery"`, or `{"judged": "keyless" \| "unrecognised" \| {"resolved": key}}`                                                                                                                                             |
| A condition read         | `reading`: `"valid"`, `"unknown"`, `"invalid"`; each problem's `location`, `code`, `scope`, and `expected_shape` where it has one                                                                                            |
| A record's key           | `{"method": …}`, `{"host": …}`, `{"path_prefix": "…"}`, `{"operation": <a record's path>}`, `{"path": <a record's path>}`                                                                                                    |
| A record's measure       | `{"size_in_bytes": [<a record's path>, …]}`, `{"is_set": [<a record's path>, …]}`, one per path                                                                                                                              |
| Taken, and why not       | `{"entry": id}`, `{"catch_all": id, "why": …}`: `"no_entry"`, `{"unmatchable": why}`, `{"refused": {entry, location, refusal}}`, `{"stopped": …}`, `{"ambiguous": {entry}}`, `{"body_unread": {entry, why}}`                 |

Resolving a path refuses only `budget_exceeded`; `unknown_op`, `invalid` and `not_for_use` come from a compiled
condition, as evaluated.

A list's items are written as a body and read as one, so an item that can't be told is a string that doesn't decode, and
one past a double is written past it.

Layer 10's events: `{"hold": {plan, at}}` and `{"record": {plan, at, call}}`, `plan` `null` for none; and
`{"count_dropped": n}`. Each one's output: a `hold`'s or a `record`'s the window it closed (`closed`: its plan, `start`,
`end`, `calls_at`, `calls_dropped`; `null` for none); a `record`'s the call's `outcome` (`"dropped"`, or
`{"counted": {"body_unread": reason}}`, the reason as a document's `truncated.body_unread` names it, `null` for a body
read or no entry's); and every event's `open_until`, when the open window ends.

Layer 9's `sent` is what the sender gives the document: `document_id`, `generated_at`, `windows_dropped`; each of its
`calls` is `{"at": …, "call": …}`.

### Order, and what a layer leaves to another

- **Layer 6** compiles a one-entry plan, which puts the request line's conditions before the body's; within a step limit
  that order decides whether the body is read at all; without one, it decides only which of two conditions that can't be
  told is the answer, both being the body's. An entry compiling refuses is matching's (layer 8): a walk never answers
  it.
- **Layer 5** wraps the op in a condition on `ticket.value` for a gateway policy, never resolved; a refusal comes before
  the value is looked at.
- **What stays a Rust test,** not a vector: the Rust API (types, sharing, never panicking); this reader's own limits'
  arithmetic (a vector pins a boundary's outcome, not its charges); invariants over generated inputs; log lines and an
  error's wording; layers below these (the body's tree, an authority refused), but where a layer above reads their
  outcome; the step budget shared by matching and its record, and a walk stopped by it, since layer 8 takes no step
  limit.

## Where the ported files come from

What changed on the way:

- **`authority.json`:** RailMon's host spelling (`schemas/vectors/destination-hosts.json`, datrail/railmon#173 at
  `3c9cb74`), each case given the `port` this reader keeps beside the host; a `port-dropped` id means dropped from the
  host, the port still read. Like the source, only authorities every reader accepts: what this reader refuses, RailMon
  reads as a host, so those stay a Rust test.
- **`redaction.json`:** RailMon's rule (`asp_redaction`, `REDACTION_VERSION` 1), its tests written as paths; a change to
  the rule updates both. Shapes whose shortest key is 20 characters or more are redacted by length here, whatever their
  shape.
- **`policy/bundle.json`:** bundles filled in to the shape Rail Center publishes; the gateway's other expectations
  (names, conditions, actions, bindings, `ticket_fallback`) aren't asserted. Removed: 24 cases this reader answers
  otherwise, all input Rail Center doesn't publish or a check made elsewhere (an `enabled` missing or not a boolean; a
  priority written as a float; a name missing or not a string; an id of a shape this reader refuses and another reads:
  odd hyphens, a leading minus, a braced URN, an upper-case URN prefix on an undashed id; a duplicate id beside a
  disabled twin, or a bad id or priority on a disabled policy; a binding naming an unknown policy; an endpoint key not
  of the form `<slug>#<path>#<method>#<call>`; a `content_hash` this reader reads opaque; a major version other than 2,
  which the caller checks).
- **`policy/judge.json`:** the whole-bundle cases of `decide.json` and those of `fallback.json`, bundles filled in to
  2.0, both fallbacks `pass` unless a case is about one, a ticket the claims Rail Center mints where a case gives none;
  `fallback.json`'s `<slug>.<tool>` keys rewritten `<slug>#/mcp#tools/call#<tool>`, its cases with no chain given one
  inert alert. An `endpoint` is `keyless`, `unrecognised` or `{"resolved": key}`, the key without its slug. A chain is
  `null` where a fallback denies at enforce; a refusal names the policy it was reached at. Removed: 5 with an action
  other than block or alert, 2 with an `enabled` missing or not a boolean (this reader refuses the bundle; Rail Center
  publishes neither), 2 with mode none, where the caller never judges.
- **`policy/base64_ticket.json`:** verbatim; a valid case's token is the payload as sent. Removed: 8 about how the
  header arrives, which the runtime handles, and 4 about a nesting cap of 64, where this reader's is 128; the contract
  leaves the cap to each reader.
- **`policy/jwt_ticket.json`:** the file's headers verbatim, the ticket's reading asserted, not the source's decision; a
  valid case's token is the payload as sent; an `added` case is signed with the same key, whose seed the source
  publishes. The file's reader is its `kid` and `public_key`, the key it holds, and its `issuer` and `audience`; a
  case's `reader` replaces it: `holds_keys` false is an empty key set, a `null` issuer or audience one not configured.
  Removed: 2 about requiring signatures, which the runtime decides. The keys sign nothing but these cases.
- **`capture/plan.json`:** the worked example, a record's `measure` named `measures`, its paths under `request`; each
  case that plan with its RFC 6902 `patch` applied (add, remove, replace). A field the reader doesn't know is passed
  over. A named context key's `function` is `none` (also absent or `null`) or `sha256`; a built-in key takes none. The
  proxy's ceilings: `window_seconds` 60 to 86400, `max_groups_per_window` up to 100000, `max_steps_per_window` up to
  1000000, `max_unsent_windows` up to 1000. Not here, since they're the caller's to check: the `content_hash`'s form,
  the `schema_version`'s major, the agent, the `redaction_version`.

Layers 3 to 5 hold, among their own, the condition cases of datrail/gateway's
`gateway-core/tests/vectors/v2/decide.json` at `041c92f`, each split into its path, its reading and its op. Three of
them, an operand past a double, are refused here as read.

## Queries

The files are JSON, so a question about the behaviours is a query:

```sh
# Every op on a list, and what it answers
jq -c '.cases[] | select(.value.list?) | {id, op, operand, expect}' vectors/condition/op.json
# Every case where a value can't be told
jq -c '.cases[] | select(.expect | tostring | test("ambiguous")) | .id' vectors/condition/path.json
# Every bundle-language path case: ticket, headers, endpoint key
jq -c '.cases[] | select(.path | tostring | test("ticket|headers|endpoint_key")) | .id' vectors/condition/path.json
```

`condition/condition.json` holds a lone surrogate and numbers past a double on purpose, which `jq` refuses; read it with
Python's `json`.
