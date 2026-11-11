# rail-policy-py

The rules language as a Python package, `datrail-policy`, for Rail Center, to check each rule before publishing it, and
for RailDash, to try a rule on a sample call. Both then run the very code the gateways enforce rules with, instead of a
Python copy that could drift from it. See the [main README](../../README.md) for the system around it.

It's the Python binding of [`rail-policy`](../rail-policy/README.md), built with maturin into the `datrail-policy`
wheel, for CPython 3.12 or later.

## Entry points

- **`validate_bundle`** says whether a policy bundle is usable;
- **`check_condition`** reports every problem with a condition, and whether every reader refuses it or only Rail Center;
- **`compile_bundle`** and **`judge_sample`** judge a sample call against a bundle and trace the policies evaluated;
- **`read_ticket`** reads an `x-rail` value into the claims `judge_sample` takes.

Each result but the compiled bundle is a frozen dataclass with a tag field (`kind`, `decision`, `verdict`), and fields
that don't apply are `None`.

## Install

No wheel is published yet. From a checkout, with a Rust toolchain:

```sh
pip install crates/rail-policy-py
```

## Usage

```python
import json

from datrail_policy import check_condition, compile_bundle, judge_sample

condition = {"path": "ticket.posture.overall.score", "op": "lt", "value": 50}
assert check_condition(condition).problems == ()

bundle = compile_bundle(
    json.dumps(
        {
            "schema_version": "2.0",
            "content_hash": "sample",
            "enforcement": {"mode": "enforce"},
            "policies": [
                {
                    "id": "5c8f1e42-0000-4000-8000-000000000001",
                    "name": "Low posture",
                    "priority": 1,
                    "condition": condition,
                    "action": "block",
                    "enabled": True,
                }
            ],
            "bindings": [],
            "binding_fallback": "pass",
            "ticket_fallback": "pass",
        }
    )
)
judgement = judge_sample(
    bundle,
    endpoint_key="/mcp#tools/call#search",
    claims={"posture": {"overall": {"score": 30, "level": "low"}}},
)
print(judgement.chain.decision)  # denied
```

## What it doesn't do

- **No fetching:** it reads what it's given, a bundle or a ticket as text; Rail Center has them already.
- **No call body:** `judge_sample` takes the endpoint key, not the request, for now.

## Development

`make test-py`, from the repository root, builds the wheel and runs its tests; it needs
[uv](https://docs.astral.sh/uv/).
