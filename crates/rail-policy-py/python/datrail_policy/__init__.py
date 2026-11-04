import json
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from typing import Any, Literal

from datrail_policy import _native
from datrail_policy._native import DEFAULT_STEP_LIMIT, CompiledBundle

_EPOCH = datetime(1970, 1, 1, tzinfo=UTC)

__all__ = [
    "DEFAULT_STEP_LIMIT",
    "BundleCheck",
    "ChainVerdict",
    "CompiledBundle",
    "Judgement",
    "NotJudged",
    "Resolved",
    "TicketReading",
    "TraceStep",
    "UnreadableSample",
    "UnusableBundle",
    "compile_bundle",
    "judge_sample",
    "read_ticket",
    "validate_bundle",
]


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
    policy: str
    # `None` when no path was resolved: the condition can't be read, or the budget ran out first.
    resolved: Resolved | None
    result: Literal["holds", "does_not_hold", "refused"]


@dataclass(frozen=True)
class ChainVerdict:
    decision: Literal["allowed", "denied", "refused"]
    # The deciding policy; `None` when allowed.
    policy: str | None
    refusal: Literal["uninterpretable", "budget_exceeded"] | None
    alerts: tuple[str, ...]


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
class TicketReading:
    state: Literal[
        "absent", "undecodable", "bad_signature", "malformed", "wrong_audience", "expired", "valid"
    ]
    # The payload as sent, for `judge_sample`; `None` unless `valid`.
    claims: dict[str, Any] | None
    kind: Literal["ticket_reading"] = "ticket_reading"


def validate_bundle(json: str) -> BundleCheck:
    problem = _native.validate_bundle(json)
    return BundleCheck(usable=problem is None, problem=problem)


def compile_bundle(json: str) -> CompiledBundle | UnusableBundle:
    compiled = _native.compile_bundle(json)
    if isinstance(compiled, str):
        return UnusableBundle(problem=compiled)
    return compiled


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
    if claims is not None and not isinstance(claims, Mapping):
        raise TypeError(f"claims must be a mapping, not {type(claims).__name__}")
    claims_text = None
    if claims is not None:
        try:
            claims_text = json.dumps(dict(claims), allow_nan=False)
        except ValueError as error:
            return UnreadableSample(problem=f"claims: {error}")
    result = json.loads(
        _native.judge_sample(
            bundle,
            endpoint_key=endpoint_key,
            headers=list(headers),
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
        chain=None
        if chain is None
        else ChainVerdict(**{**chain, "alerts": tuple(chain["alerts"])}),
        trace=tuple(_read_step(step) for step in result["trace"]),
    )


def _read_step(step: dict[str, Any]) -> TraceStep:
    resolved = step["resolved"]
    return TraceStep(
        policy=step["policy"],
        resolved=None if resolved is None else Resolved(**resolved),
        result=step["result"],
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
    return TicketReading(state=reading["state"], claims=reading["claims"])
