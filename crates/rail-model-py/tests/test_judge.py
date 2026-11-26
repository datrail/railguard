import base64
import json
import math
from datetime import UTC, datetime
from pathlib import Path
from uuid import UUID

import pytest

from datrail_model import (
    DEFAULT_STEP_LIMIT,
    ChainOutcome,
    CompiledBundle,
    Judgement,
    NotJudged,
    ResolvedValue,
    TraceStep,
    UnreadableSample,
    UnusableBundle,
    compile_bundle,
    judge_sample,
    read_ticket,
)

_VECTORS = Path(__file__).parents[3] / "vectors"
_WALK_CASES = json.loads((_VECTORS / "policy" / "judge.json").read_text())["cases"]
_ENVIRONMENTS = json.loads((_VECTORS / "environments.json").read_text())["environments"]
# Layer 3's paths a policy reads, in an environment a sample can be.
_PATH_CASES = [
    case
    for case in json.loads((_VECTORS / "condition" / "path.json").read_text())["cases"]
    if case["use"] == "gateway_policy" and _ENVIRONMENTS[case["environment"]]["for"] == "judging"
]
_FOUND_KINDS = {"string", "number", "bool", "list"}


def _build_policy(n, condition, action="block", enabled=True):
    return {
        "id": str(_get_policy_id(n)),
        "name": f"P-{n}",
        "priority": n,
        "condition": condition,
        "action": action,
        "enabled": enabled,
    }


def _compile_bundle(policies, *, mode="enforce", bindings=(), ticket_fallback="pass"):
    compiled = compile_bundle(
        json.dumps(
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
    return UUID(f"5c8f1e42-0000-4000-8000-0000000000{n:02}")


def _format_outcome(judgement):
    # In the walk vectors' own shape, so a failure shows both sides alike.
    assert isinstance(judgement, Judgement), judgement
    chain = judgement.chain
    if chain is not None:
        decision = (
            chain.decision if chain.decision == "allowed" else {chain.decision: str(chain.policy)}
        )
        chain = {"decision": decision, "alerts": [str(alert) for alert in chain.alerts]}
    return {"fallbacks": list(judgement.fallback_denials), "chain": chain}


def _get_expected_chain(expect):
    # `existing` holds on whatever is found, so the block policy denies; a refusal refuses the call.
    if expect == "absent":
        return "allowed", None
    if expect == "object" or (isinstance(expect, dict) and expect.keys() <= _FOUND_KINDS):
        return "denied", None
    if isinstance(expect, dict) and expect.keys() <= {"refused_reading", "refused"}:
        return "refused", next(iter(expect.values()))
    raise AssertionError(f"no decision for {expect!r}")


def _get_resolved_kind(expect):
    if isinstance(expect, str):
        return expect
    kind = next(iter(expect))
    # A refused path resolves to nothing.
    return None if kind.startswith("refused") else kind


def _get_step_limit(case):
    if "step_limit" not in case:
        return DEFAULT_STEP_LIMIT
    # The condition's own step and one per list item, so only the path's cost is left to the case.
    expect = case["expect"]
    items = expect["list"] if isinstance(expect, dict) and "list" in expect else []
    return case["step_limit"] + 1 + len(items)


def test_has_cases():
    assert _WALK_CASES
    assert _PATH_CASES


@pytest.mark.parametrize("case", _WALK_CASES, ids=lambda case: case["id"])
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


@pytest.mark.parametrize("case", _PATH_CASES, ids=lambda case: case["id"])
def test_judges_each_path_as_layer_3_resolves_it(case):
    environment = _ENVIRONMENTS[case["environment"]]
    endpoint = environment["endpoint"]
    compiled = _compile_bundle([_build_policy(1, {"path": case["path"], "op": "existing"})])

    judgement = judge_sample(
        compiled,
        endpoint_key=endpoint if isinstance(endpoint, str) else endpoint["resolved"],
        headers=[tuple(line) for line in environment["headers"]],
        claims=environment["ticket"],
        step_limit=_get_step_limit(case),
    )

    decision, refusal = _get_expected_chain(case["expect"])
    assert (judgement.chain.decision, judgement.chain.refusal) == (decision, refusal)
    resolved = judgement.trace[0].resolved
    assert (None if resolved is None else resolved.kind) == _get_resolved_kind(case["expect"])


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
        fallback_denials=(),
        chain=ChainOutcome(
            decision="denied", policy=_get_policy_id(3), refusal=None, alerts=(_get_policy_id(1),)
        ),
        trace=(
            TraceStep(
                policy=_get_policy_id(1),
                resolved=ResolvedValue(kind="bool", value=True),
                verdict="holds",
                refusal=None,
            ),
            TraceStep(
                policy=_get_policy_id(2),
                resolved=ResolvedValue(kind="absent"),
                verdict="does_not_hold",
                refusal=None,
            ),
            TraceStep(
                policy=_get_policy_id(3),
                resolved=ResolvedValue(kind="list", value=["b", "c"]),
                verdict="holds",
                refusal=None,
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
        ResolvedValue(kind="string", value="a"),
        ResolvedValue(kind="number", value=1.5),
        ResolvedValue(kind="object"),
        ResolvedValue(kind="string", value="/mcp#tools/call#t"),
        ResolvedValue(kind="bool", value=False),
    ]


def test_traces_a_refusal_with_nothing_resolved():
    compiled = _compile_bundle(
        [
            _build_policy(1, {"path": "ticket.score", "op": "lt", "value": "x"}),
            _build_policy(2, {"path": "ticket.score", "op": "lt", "value": 40}),
        ]
    )

    judgement = judge_sample(compiled, endpoint_key="keyless", claims={"score": 10})

    assert judgement.chain == ChainOutcome(
        decision="refused", policy=_get_policy_id(1), refusal="invalid", alerts=()
    )
    assert judgement.trace == (
        TraceStep(
            policy=_get_policy_id(1),
            resolved=None,
            verdict="refused",
            refusal="invalid",
        ),
    )


@pytest.mark.parametrize(
    ("condition", "refusal"),
    [
        ({"path": "ticket.score", "op": "newer_op", "value": 1}, "unknown_op"),
        ({"path": "request.host", "op": "existing"}, "not_for_use"),
    ],
)
def test_refuses_with_each_cause(condition, refusal):
    compiled = _compile_bundle([_build_policy(1, condition)])
    judgement = judge_sample(compiled, endpoint_key="keyless", claims={"score": 10})
    assert judgement.chain.refusal == refusal


def test_refuses_past_the_step_limit():
    compiled = _compile_bundle(
        [_build_policy(n, {"path": "ticket.score", "op": "gt", "value": 40}) for n in (1, 2)]
    )

    judgement = judge_sample(compiled, endpoint_key="keyless", claims={"score": 10}, step_limit=1)

    assert judgement.chain == ChainOutcome(
        decision="refused", policy=_get_policy_id(2), refusal="budget_exceeded", alerts=()
    )
    assert judgement.trace[-1] == TraceStep(
        policy=_get_policy_id(2), resolved=None, verdict="refused", refusal="budget_exceeded"
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
            resolved=ResolvedValue(kind="list", value=["a", "b", "c"]),
            verdict="refused",
            refusal="budget_exceeded",
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
    assert observed.fallback_denials == ("ticket",)
    assert observed.chain is not None
    assert enforced == Judgement(mode="enforce", fallback_denials=("ticket",), chain=None, trace=())


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


def test_judges_the_claims_read_from_a_ticket():
    compiled = _compile_bundle(
        [_build_policy(1, {"path": "ticket.score", "op": "lt", "value": 40})]
    )
    ticket = base64.urlsafe_b64encode(json.dumps({"exp": 100, "score": 10}).encode()).decode()
    reading = read_ticket(ticket, now=datetime.fromtimestamp(0, UTC))

    judgement = judge_sample(compiled, endpoint_key="keyless", claims=reading.claims)

    assert judgement.chain.decision == "denied"
