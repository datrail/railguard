import json
import math
from decimal import Decimal
from pathlib import Path

import pytest

from datrail_policy import (
    ChainVerdict,
    CompiledBundle,
    Judgement,
    NotJudged,
    Resolved,
    TraceStep,
    UnreadableSample,
    UnusableBundle,
    compile_bundle,
    judge_sample,
)

_VECTORS = Path(__file__).parents[3] / "vectors"
_WALK_CASES = json.loads((_VECTORS / "judge_walk_conformance_cases.json").read_text())["cases"]
# Decimal, so an operand past what a double holds is written back as sent.
_CONDITION_CASES = json.loads(
    (_VECTORS / "judge_condition_conformance_cases.json").read_text(), parse_float=Decimal
)["cases"]
_CONDITION_OUTCOMES = {"holds": "denied", "does_not_hold": "allowed", "refused": "refused"}


def _dump_json(value):
    if isinstance(value, Decimal):
        return str(value)
    if isinstance(value, dict):
        return "{" + ",".join(f"{json.dumps(k)}:{_dump_json(v)}" for k, v in value.items()) + "}"
    if isinstance(value, list):
        return "[" + ",".join(_dump_json(item) for item in value) + "]"
    return json.dumps(value)


def _build_policy(n, condition, action="block", enabled=True):
    return {
        "id": f"5c8f1e42-0000-4000-8000-0000000000{n:02}",
        "name": f"P-{n}",
        "priority": n,
        "condition": condition,
        "action": action,
        "enabled": enabled,
    }


def _compile_bundle(policies, *, mode="enforce", bindings=(), ticket_fallback="pass"):
    compiled = compile_bundle(
        _dump_json(
            {
                "schema_version": "2.0",
                "content_hash": "wheel",
                "enforcement": {"mode": mode},
                "policies": policies,
                "bindings": list(bindings),
                "binding_fallback": "pass",
                "ticket_fallback": ticket_fallback,
            }
        )
    )
    assert isinstance(compiled, CompiledBundle), compiled
    return compiled


def _get_policy_id(n):
    return f"5c8f1e42-0000-4000-8000-0000000000{n:02}"


def _format_outcome(judgement):
    # In the walk vectors' own shape, so a failure shows both sides alike.
    assert isinstance(judgement, Judgement), judgement
    chain = judgement.chain
    if chain is not None:
        decision = chain.decision if chain.decision == "allowed" else {chain.decision: chain.policy}
        chain = {"decision": decision, "alerts": list(chain.alerts)}
    return {"fallbacks": list(judgement.fallbacks), "chain": chain}


def test_has_cases():
    assert _WALK_CASES
    assert _CONDITION_CASES


@pytest.mark.parametrize("case", _WALK_CASES, ids=lambda case: case["name"])
def test_walk_vectors(case):
    endpoint = case["endpoint"]
    judgement = judge_sample(
        compile_bundle(json.dumps(case["bundle"])),
        endpoint_key=endpoint if isinstance(endpoint, str) else endpoint["resolved"],
        headers=[tuple(line) for line in case["headers"]],
        claims=case["ticket"],
        mode=case["mode"],
    )

    assert _format_outcome(judgement) == case["expect"]


@pytest.mark.parametrize("case", _CONDITION_CASES, ids=lambda case: case["name"])
def test_condition_vectors(case):
    judgement = judge_sample(
        _compile_bundle([_build_policy(1, case["condition"])]),
        endpoint_key=case["endpoint_key"],
        headers=[tuple(line) for line in case["headers"]],
        # Claims hold no number past a double, so they can go back to floats.
        claims=None if case["ticket"] is None else json.loads(_dump_json(case["ticket"])),
    )

    assert isinstance(judgement, Judgement), judgement
    assert judgement.chain.decision == _CONDITION_OUTCOMES[case["expect"]]


def test_traces_each_policy_evaluated_until_one_decides():
    compiled = _compile_bundle(
        [
            _build_policy(1, {"path": "ticket.admin", "op": "existing"}, action="alert"),
            _build_policy(2, {"path": "ticket.missing", "op": "existing"}),
            _build_policy(3, {"path": "request.headers.x-a", "op": "contains", "value": "c"}),
            _build_policy(4, {"path": "ticket.score", "op": "lt", "value": 40}),
        ]
    )

    judgement = judge_sample(
        compiled,
        endpoint_key="/mcp#tools/call#t",
        headers=[("X-A", "b, c")],
        claims={"admin": True, "score": 10},
    )

    assert judgement == Judgement(
        mode="enforce",
        fallbacks=(),
        chain=ChainVerdict(
            decision="denied", policy=_get_policy_id(3), refusal=None, alerts=(_get_policy_id(1),)
        ),
        trace=(
            TraceStep(
                policy=_get_policy_id(1), resolved=Resolved(kind="bool", value=True), result="held"
            ),
            TraceStep(
                policy=_get_policy_id(2), resolved=Resolved(kind="absent"), result="did_not_hold"
            ),
            TraceStep(
                policy=_get_policy_id(3),
                resolved=Resolved(kind="list", value=["b", "c"]),
                result="held",
            ),
        ),
    )


def test_traces_what_each_path_resolved_to():
    conditions = [
        {"path": "ticket.name", "op": "eq", "value": "x"},
        {"path": "ticket.score", "op": "eq", "value": 1},
        {"path": "ticket.posture", "op": "eq", "value": 1},
        {"path": "request.endpoint_key", "op": "eq", "value": 1},
        {"path": "ticket.flag", "op": "eq", "value": 1},
    ]
    compiled = _compile_bundle(
        [_build_policy(n, c, action="alert") for n, c in enumerate(conditions, 1)]
    )

    judgement = judge_sample(
        compiled,
        endpoint_key="/mcp#tools/call#t",
        claims={"name": "a", "score": 1.5, "posture": {}, "flag": False},
    )

    assert [step.resolved for step in judgement.trace] == [
        Resolved(kind="string", value="a"),
        Resolved(kind="number", value=1.5),
        Resolved(kind="object"),
        Resolved(kind="string", value="/mcp#tools/call#t"),
        Resolved(kind="bool", value=False),
    ]


def test_traces_a_refusal_with_nothing_resolved():
    compiled = _compile_bundle(
        [
            _build_policy(1, {"path": "ticket.score", "op": "lt", "value": "x"}),
            _build_policy(2, {"path": "ticket.score", "op": "lt", "value": 40}),
        ]
    )

    judgement = judge_sample(compiled, endpoint_key="keyless", claims={"score": 10})

    assert judgement.chain == ChainVerdict(
        decision="refused", policy=_get_policy_id(1), refusal="uninterpretable", alerts=()
    )
    assert judgement.trace == (
        TraceStep(policy=_get_policy_id(1), resolved=None, result="refused"),
    )


def test_leaves_out_of_the_trace_what_a_binding_excludes():
    compiled = _compile_bundle(
        [
            _build_policy(1, {"path": "ticket.score", "op": "gt", "value": 40}),
            _build_policy(2, {"path": "ticket.score", "op": "gt", "value": 40}),
        ],
        bindings=[
            {
                "endpoint_key": "slug#/mcp#tools/call#t",
                "mode": "gated",
                "policy_ids": [_get_policy_id(2)],
            }
        ],
    )

    judgement = judge_sample(compiled, endpoint_key="/mcp#tools/call#t", claims={"score": 10})

    assert [step.policy for step in judgement.trace] == [_get_policy_id(2)]


def test_refuses_past_the_step_limit():
    compiled = _compile_bundle(
        [_build_policy(n, {"path": "ticket.score", "op": "gt", "value": 40}) for n in (1, 2)]
    )

    judgement = judge_sample(compiled, endpoint_key="keyless", claims={"score": 10}, step_limit=1)

    assert judgement.chain == ChainVerdict(
        decision="refused", policy=_get_policy_id(2), refusal="budget_exceeded", alerts=()
    )
    assert judgement.trace[-1] == TraceStep(
        policy=_get_policy_id(2), resolved=None, result="refused"
    )


def test_traces_a_path_resolved_before_the_budget_ran_out():
    compiled = _compile_bundle(
        [_build_policy(1, {"path": "request.headers.x-a", "op": "contains", "value": "c"})]
    )

    # One step for the condition, then three for the list: past a limit of 2.
    judgement = judge_sample(
        compiled, endpoint_key="keyless", headers=[("x-a", "a, b, c")], step_limit=2
    )

    assert judgement.trace == (
        TraceStep(
            policy=_get_policy_id(1),
            resolved=Resolved(kind="list", value=["a", "b", "c"]),
            result="refused",
        ),
    )
    assert judgement.chain.refusal == "budget_exceeded"


def test_mode_defaults_to_the_bundle_and_can_be_overridden():
    compiled = _compile_bundle(
        [_build_policy(1, {"path": "ticket.score", "op": "gt", "value": 40})],
        mode="observe",
        ticket_fallback="block",
    )

    observed = judge_sample(compiled, endpoint_key="keyless")
    enforced = judge_sample(compiled, endpoint_key="keyless", mode="enforce")

    assert observed.mode == "observe"
    assert observed.fallbacks == ("ticket",)
    assert observed.chain is not None
    assert enforced == Judgement(mode="enforce", fallbacks=("ticket",), chain=None, trace=())


def test_judges_nothing_at_mode_none():
    compiled = _compile_bundle([], mode="none")

    assert judge_sample(compiled, endpoint_key="keyless") == NotJudged(reason="mode_none")
    assert isinstance(judge_sample(compiled, endpoint_key="keyless", mode="enforce"), Judgement)


@pytest.mark.parametrize(
    ("endpoint_key", "claims", "problem"),
    [
        ("/mcp#tools/call", None, 'endpoint key "/mcp#tools/call": not <path>#<method>#<call>'),
        ("a\nb", None, 'endpoint key "a\\u{a}b"'),
        ("keyless", {"score": math.nan}, "claims: Out of range float values"),
        ("keyless", {"score": math.inf}, "claims: Out of range float values"),
        ("keyless", {"score": 10**400}, "claims: number out of range"),
    ],
    ids=["two parts", "a newline", "nan", "infinity", "past a double"],
)
def test_reads_an_unreadable_sample_as_such(endpoint_key, claims, problem):
    judgement = judge_sample(_compile_bundle([]), endpoint_key=endpoint_key, claims=claims)

    assert isinstance(judgement, UnreadableSample)
    assert judgement.problem.startswith(problem), judgement.problem


def test_explains_a_long_endpoint_key_after_cutting_it():
    judgement = judge_sample(_compile_bundle([]), endpoint_key="a" * 300)

    assert judgement.problem.endswith('…": not <path>#<method>#<call>, keyless or unrecognised')


def test_reads_an_unreadable_sample_whatever_the_mode():
    compiled = _compile_bundle([], mode="none")

    assert isinstance(judge_sample(compiled, endpoint_key="x"), UnreadableSample)
    assert isinstance(
        judge_sample(compiled, endpoint_key="keyless", claims={"a": 10**400}), UnreadableSample
    )
    with pytest.raises(ValueError, match="mode"):
        judge_sample(compiled, endpoint_key="keyless", claims={"a": math.nan}, mode="bogus")


def test_reads_an_unusable_bundle_as_such():
    assert compile_bundle("{}") == UnusableBundle(
        problem="unusable policy bundle: missing field `schema_version` at line 1 column 2"
    )


@pytest.mark.parametrize(
    ("arguments", "error"),
    [
        ({"mode": "none"}, ValueError),
        ({"mode": "Enforce"}, ValueError),
        ({"claims": [("a", 1)]}, TypeError),
        ({"headers": [("a", 1)]}, TypeError),
        ({"headers": "ab"}, TypeError),
        ({"step_limit": -1}, OverflowError),
    ],
)
def test_raises_on_the_wrong_type(arguments, error):
    with pytest.raises(error):
        judge_sample(_compile_bundle([]), endpoint_key="keyless", **arguments)


def test_raises_on_a_bundle_not_compiled():
    with pytest.raises(TypeError):
        judge_sample("{}", endpoint_key="keyless")
