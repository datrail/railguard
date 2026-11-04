import json
import math
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from typing import Any, Literal
from uuid import UUID

from datrail_policy import _native
from datrail_policy._native import DEFAULT_STEP_LIMIT, CompiledBundle

_EPOCH = datetime(1970, 1, 1, tzinfo=UTC)
# Past what `rail-policy` reads (128), and short of `json.dumps`'s recursion limit.
_MAX_WALK_DEPTH = 200

__all__ = [
    "DEFAULT_STEP_LIMIT",
    "TICKET_SCHEMA",
    "BundleCheck",
    "ChainVerdict",
    "ConditionCheck",
    "ConditionProblem",
    "CompiledBundle",
    "Judgement",
    "NotJudged",
    "Resolved",
    "TicketReading",
    "TraceStep",
    "UnreadableSample",
    "UnusableBundle",
    "check_condition",
    "compile_bundle",
    "judge_sample",
    "read_ticket",
    "validate_bundle",
]


# Rail Center's `_TICKET_SCHEMA`; read at call time, so a library can replace it.
TICKET_SCHEMA: dict[str, Any] = {
    "id": "string",
    "agent_id": "string",
    "sandbox_id": "string",
    "sandbox_type": "string",
    "environment_fingerprint": "string",
    "skills": "list",
    "posture": {
        "overall": {"score": "number", "level": "string"},
        "categories": {
            name: {"score": "number", "capped": "boolean"}
            for name in (
                "identity",
                "api_access",
                "data_reach",
                "containment",
                "data_flow",
                "injection_exposure",
                "grant_exercise_gap",
                "text_signals",
            )
        },
    },
    "alignment": {"baseline": {"id": "string", "label": "string"}, "drifted": "list"},
}


@dataclass(frozen=True)
class BundleCheck:
    usable: bool
    problem: str | None


@dataclass(frozen=True)
class UnusableBundle:
    problem: str
    kind: Literal["unusable_bundle"] = "unusable_bundle"


@dataclass(frozen=True)
class Resolved:
    kind: Literal["absent", "string", "number", "bool", "list", "object"]
    # The value as JSON; `None` for `absent` and `object`.
    value: Any = None


@dataclass(frozen=True)
class TraceStep:
    policy: UUID
    # `None` when no path was resolved: the condition can't be read, or the budget ran out first.
    resolved: Resolved | None
    verdict: Literal["holds", "does_not_hold", "refused"]
    # Why the condition refused; `None` unless `refused`.
    refusal: Literal["uninterpretable", "budget_exceeded"] | None


@dataclass(frozen=True)
class ChainVerdict:
    decision: Literal["allowed", "denied", "refused"]
    # The deciding policy; `None` when allowed.
    policy: UUID | None
    refusal: Literal["uninterpretable", "budget_exceeded"] | None
    alerts: tuple[UUID, ...]


@dataclass(frozen=True)
class Judgement:
    mode: Literal["observe", "enforce"]
    fallbacks: tuple[Literal["ticket", "binding"], ...]
    # `None` when the policies weren't consulted: at `enforce`, after a fallback denied.
    chain: ChainVerdict | None
    # The policies the walk evaluated, in order; any other policy didn't run.
    trace: tuple[TraceStep, ...]
    kind: Literal["judged"] = "judged"


@dataclass(frozen=True)
class NotJudged:
    reason: Literal["mode_none"]
    kind: Literal["not_judged"] = "not_judged"


@dataclass(frozen=True)
class UnreadableSample:
    problem: str
    kind: Literal["unreadable_sample"] = "unreadable_sample"


@dataclass(frozen=True)
class ConditionProblem:
    # A JSON pointer into the condition.
    pointer: str
    code: Literal[
        "not_an_object",
        "unknown_source",
        "unknown_op",
        "operand_shape",
        "unknown_path",
        "op_not_for_path",
        "unwritable_number",
        "too_deep",
        "unreadable",
    ]
    # `enforcement`: every reader refuses it. `authoring`: only the writer, since a reader may be
    # on another version.
    scope: Literal["enforcement", "authoring"]
    # The operand shape the op takes; `None` unless `operand_shape`.
    expected: Literal["number", "list", "scalar", "string"] | None
    message: str


@dataclass(frozen=True)
class ConditionCheck:
    # Empty when every reader reads the condition and Rail Center would publish it.
    problems: tuple[ConditionProblem, ...]


@dataclass(frozen=True)
class TicketReading:
    state: Literal[
        "absent", "undecodable", "bad_signature", "malformed", "wrong_audience", "expired", "valid"
    ]
    # The payload as sent, for `judge_sample`; `None` unless `valid`.
    claims: dict[str, Any] | None


def validate_bundle(json: str) -> BundleCheck:
    problem = _native.validate_bundle(json)
    return BundleCheck(usable=problem is None, problem=problem)


def compile_bundle(json: str) -> CompiledBundle | UnusableBundle:
    compiled = _native.compile_bundle(json)
    if isinstance(compiled, str):
        return UnusableBundle(problem=compiled)
    return compiled


def check_condition(
    condition: Any, *, ticket_schema: Mapping[str, Any] | None = None
) -> ConditionCheck:
    """Every problem with a condition, path first, then op, then value.

    `ticket_schema` defaults to `TICKET_SCHEMA`. A malformed one raises `ValueError`.
    """
    schema_text = _write_schema(TICKET_SCHEMA if ticket_schema is None else ticket_schema)
    # JSON can't carry them to Rust, so a stand-in keeps the other checks running.
    non_finite: list[str] = []
    try:
        text = json.dumps(_replace_non_finite(condition, non_finite))
    except _TooDeep:
        return ConditionCheck(problems=(_build_unreadable("nested too deep"),))
    except ValueError as error:
        # Such as an int past Python's digit limit.
        return ConditionCheck(problems=(_build_unreadable(str(error)),))
    native = _native.check_condition(text, ticket_schema=schema_text)
    problems = [ConditionProblem(**problem) for problem in json.loads(native)]
    problems += [
        ConditionProblem(
            pointer=pointer,
            code="unwritable_number",
            scope="authoring",
            expected=None,
            message="infinity or NaN can't be written back as the JSON it came from",
        )
        for pointer in non_finite
    ]
    return ConditionCheck(problems=tuple(problems))


class _TooDeep(Exception):
    pass


def _replace_non_finite(condition: Any, found: list[str]) -> Any:
    """A copy of plain dicts and lists, with each non-finite float's pointer in `found`."""
    root = [condition]
    stack: list[tuple[Any, Any, str, int]] = [(root, 0, "", 1)]
    # Iterative, since depth is one of the things checked; children pushed in reverse, so pointers
    # come in document order.
    while stack:
        parent, key, pointer, level = stack.pop()
        value = parent[key]
        if level > _MAX_WALK_DEPTH:
            raise _TooDeep
        if isinstance(value, float) and not math.isfinite(value):
            found.append(pointer)
            parent[key] = 0.0
        elif isinstance(value, Mapping):
            copy = {}
            for item_key, item in value.items():
                if not isinstance(item_key, str):
                    raise TypeError(f"condition keys must be str, not {type(item_key).__name__}")
                copy[item_key] = item
            parent[key] = copy
            pointers = [(k, f"{pointer}/{_escape_pointer(k)}") for k in copy]
            stack.extend((copy, k, child, level + 1) for k, child in reversed(pointers))
        elif isinstance(value, list | tuple):
            copy = list(value)
            parent[key] = copy
            indices = reversed(range(len(copy)))
            stack.extend((copy, i, f"{pointer}/{i}", level + 1) for i in indices)
    return root[0]


def _build_unreadable(reason: str) -> ConditionProblem:
    return ConditionProblem(
        pointer="",
        code="unreadable",
        scope="enforcement",
        expected=None,
        message=f"not JSON this reader can hold: {reason}",
    )


def _write_schema(schema: Mapping[str, Any]) -> str:
    try:
        return json.dumps(schema, default=_read_mapping)
    except (TypeError, ValueError) as error:
        raise ValueError(f"ticket schema: {error}") from None


def _read_mapping(value: Any) -> dict[str, Any]:
    if isinstance(value, Mapping):
        return dict(value)
    raise TypeError(f"{type(value).__name__} is not a kind")


def _escape_pointer(key: str) -> str:
    return key.replace("~", "~0").replace("/", "~1")


def judge_sample(
    bundle: CompiledBundle,
    *,
    endpoint_key: str,
    headers: Sequence[tuple[str, str]] = (),
    claims: Mapping[str, Any] | None = None,
    mode: Literal["observe", "enforce"] | None = None,
    step_limit: int = DEFAULT_STEP_LIMIT,
) -> Judgement | NotJudged | UnreadableSample:
    """Judges a call to `endpoint_key`: a key without its slug, `keyless` or `unrecognised`.

    `claims` is a usable ticket's payload, or `None` for no usable ticket. `mode` defaults to the
    bundle's.
    """
    # Before the claims are read, so a wrong mode raises whatever the claims hold.
    if mode not in (None, "observe", "enforce"):
        raise ValueError(f"mode {mode!r}: not observe or enforce")
    claims_text = None
    if claims is not None:
        if not isinstance(claims, Mapping):
            raise TypeError(f"claims must be a mapping, not {type(claims).__name__}")
        try:
            claims_text = json.dumps(dict(claims), allow_nan=False)
        except ValueError as error:
            return UnreadableSample(problem=f"claims: {error}")
    result = json.loads(
        _native.judge_sample(
            bundle,
            endpoint_key=endpoint_key,
            headers=headers,
            claims=claims_text,
            mode=mode,
            step_limit=step_limit,
        )
    )
    return _read_result(result)


def _read_result(result: dict[str, Any]) -> Judgement | NotJudged | UnreadableSample:
    if result["kind"] == "not_judged":
        return NotJudged(reason=result["reason"])
    if result["kind"] == "unreadable_sample":
        return UnreadableSample(problem=result["problem"])
    chain = result["chain"]
    return Judgement(
        mode=result["mode"],
        fallbacks=tuple(result["fallbacks"]),
        chain=None if chain is None else _read_chain(chain),
        trace=tuple(_read_step(step) for step in result["trace"]),
    )


def _read_chain(chain: dict[str, Any]) -> ChainVerdict:
    policy = chain["policy"]
    return ChainVerdict(
        decision=chain["decision"],
        policy=None if policy is None else UUID(policy),
        refusal=chain["refusal"],
        alerts=tuple(UUID(alert) for alert in chain["alerts"]),
    )


def _read_step(step: dict[str, Any]) -> TraceStep:
    resolved = step["resolved"]
    return TraceStep(
        policy=UUID(step["policy"]),
        resolved=None if resolved is None else Resolved(**resolved),
        verdict=step["verdict"],
        refusal=step["refusal"],
    )


def read_ticket(
    raw_ticket: str | None,
    *,
    now: datetime,
    keys: Mapping[str, bytes] | None = None,
    issuer: str | None = None,
    audience: str | None = None,
) -> TicketReading:
    """Classifies an `x-rail` value as the gateway does: a signed ticket if it holds a `.`.

    `keys` maps each `kid` to its 32-byte Ed25519 public key. A repeated header is the caller's
    to read as `undecodable`.
    """
    if not isinstance(now, datetime):
        raise TypeError(f"now must be a datetime, not {type(now).__name__}")
    if now.utcoffset() is None:
        raise ValueError("now must be timezone-aware")
    if keys is not None and not isinstance(keys, Mapping):
        raise TypeError(f"keys must be a mapping, not {type(keys).__name__}")
    reading = json.loads(
        _native.read_ticket(
            raw_ticket,
            # Exact, where `timestamp()` goes through a float.
            now_in_seconds=(now - _EPOCH) // timedelta(seconds=1),
            keys=dict(keys or {}),
            issuer=issuer,
            audience=audience,
        )
    )
    return TicketReading(**reading)
