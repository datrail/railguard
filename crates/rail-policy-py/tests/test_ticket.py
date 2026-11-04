import base64
import json
from datetime import UTC, datetime, timedelta, timezone
from pathlib import Path

import pytest

from datrail_policy import Judgement, TicketReading, compile_bundle, judge_sample, read_ticket

_VECTORS = Path(__file__).parents[3] / "vectors"
_BASE64 = json.loads((_VECTORS / "parse_base64_ticket_conformance_cases.json").read_text())
_JWT = json.loads((_VECTORS / "parse_jwt_ticket_conformance_cases.json").read_text())
_JWT_KEY = base64.urlsafe_b64decode(_JWT["public_key"] + "=")


def _get_time(seconds):
    return datetime.fromtimestamp(seconds, UTC)


def _encode_ticket(claims):
    return base64.urlsafe_b64encode(json.dumps(claims).encode()).decode()


def _check_case(case, reading):
    assert reading.state == case["state"]
    # `60` and `60.0` are one value, as rules compare them.
    assert reading.claims == case.get("token")


def test_has_cases():
    assert _BASE64["cases"]
    assert _JWT["cases"]


@pytest.mark.parametrize("case", _BASE64["cases"], ids=lambda case: case["name"])
def test_base64_vectors(case):
    reading = read_ticket(case["header"], now=_get_time(case["now_in_seconds"]))

    _check_case(case, reading)


@pytest.mark.parametrize("case", _JWT["cases"], ids=lambda case: case["name"])
def test_jwt_vectors(case):
    reader = case.get("reader") or {
        "holds_keys": True,
        "issuer": _JWT["issuer"],
        "audience": _JWT["audience"],
    }

    reading = read_ticket(
        case["header"],
        now=_get_time(case["now_in_seconds"]),
        keys={_JWT["kid"]: _JWT_KEY} if reader["holds_keys"] else None,
        issuer=reader["issuer"],
        audience=reader["audience"],
    )

    _check_case(case, reading)


def test_reads_no_header_as_absent():
    assert read_ticket(None, now=_get_time(0)) == TicketReading(state="absent", claims=None)


def test_reads_now_floored_to_whole_seconds():
    ticket = _encode_ticket({"exp": 100})

    assert read_ticket(ticket, now=_get_time(99.9)).state == "valid"
    assert read_ticket(ticket, now=_get_time(100)).state == "expired"
    # Floored, not truncated: -0.5 is -1, before `exp`.
    assert read_ticket(_encode_ticket({"exp": 0}), now=_get_time(-0.5)).state == "valid"
    # The same instant, in another zone.
    east = timezone(timedelta(hours=5))
    assert read_ticket(ticket, now=_get_time(100).astimezone(east)).state == "expired"


def test_feeds_its_claims_to_judge_sample():
    compiled = compile_bundle(
        json.dumps(
            {
                "schema_version": "2.0",
                "content_hash": "ticket",
                "enforcement": {"mode": "enforce"},
                "policies": [
                    {
                        "id": "5c8f1e42-0000-4000-8000-000000000001",
                        "name": "P-1",
                        "priority": 1,
                        "condition": {"path": "ticket.score", "op": "lt", "value": 40},
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
    reading = read_ticket(_encode_ticket({"exp": 100, "score": 10}), now=_get_time(0))

    judgement = judge_sample(compiled, endpoint_key="keyless", claims=reading.claims)

    assert isinstance(judgement, Judgement)
    assert judgement.chain.decision == "denied"


def test_raises_on_a_naive_now():
    with pytest.raises(ValueError, match="timezone-aware"):
        read_ticket(None, now=datetime(2026, 1, 1))


# Little-endian `y = 2`, for which no point is on the curve.
@pytest.mark.parametrize(
    "key", [b"short", bytes(33), b"\x02" + bytes(31)], ids=["short", "long", "off-curve"]
)
def test_raises_on_a_key_that_is_not_ed25519(key):
    with pytest.raises(ValueError, match="not an Ed25519 public key"):
        read_ticket(None, now=_get_time(0), keys={"k": key})


@pytest.mark.parametrize(
    "arguments",
    [
        {"now": 0},
        {"now": _get_time(0), "keys": {"k": "a" * 32}},
        {"now": _get_time(0), "keys": {"k": list(range(32))}},
        {"now": _get_time(0), "keys": {"k": bytearray(32)}},
        {"now": _get_time(0), "keys": [("k", bytes(32))]},
    ],
    ids=["now an int", "key a str", "key a list", "key a bytearray", "keys a list"],
)
def test_raises_on_the_wrong_type(arguments):
    with pytest.raises(TypeError):
        read_ticket(None, **arguments)
