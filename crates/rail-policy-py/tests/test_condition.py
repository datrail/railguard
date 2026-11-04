import json
import math
from pathlib import Path
from types import MappingProxyType

import pytest

import datrail_policy
from datrail_policy import ConditionCheck, ConditionProblem, check_condition

_VECTORS = Path(__file__).parents[3] / "vectors"
_CASES = json.loads((_VECTORS / "judge_condition_conformance_cases.json").read_text())["cases"]
# What makes a reader refuse the request that reaches the condition, or Rail Center refuse it.
_REFUSING_CODES = {"unknown_op", "unwritable_number"}


def _get_codes(check):
    return [(problem.location, problem.code, problem.scope) for problem in check.problems]


def test_has_cases():
    assert _CASES


@pytest.mark.parametrize("case", _CASES, ids=lambda case: case["name"])
def test_condition_vectors(case):
    check = check_condition(case["condition"])

    refused = any(
        problem.scope == "enforcement" or problem.code in _REFUSING_CODES
        for problem in check.problems
    )
    assert refused is (case["expect"] == "refused"), check


def test_accepts_a_condition_rail_center_publishes():
    condition = {"path": "ticket.posture.categories.identity.score", "op": "lt", "value": 40}

    assert check_condition(condition) == ConditionCheck(problems=())


def test_reports_a_problem_as_data():
    check = check_condition({"path": "ticket.skills", "op": "in", "value": "x"})

    assert check == ConditionCheck(
        problems=(
            ConditionProblem(
                location=("value",),
                code="operand_shape",
                scope="enforcement",
                expected_shape="list",
                message="in takes a list",
            ),
        )
    )


def test_reports_what_only_rail_center_refuses():
    check = check_condition({"path": "ticket.posture.score", "op": "lt", "value": 1})

    assert _get_codes(check) == [(("path",), "unknown_path", "authoring")]


def test_reports_a_non_object():
    assert _get_codes(check_condition(["path"])) == [((), "not_an_object", "enforcement")]


@pytest.mark.parametrize("number", [math.nan, math.inf, -math.inf])
def test_reports_each_number_that_cannot_be_written_back(number):
    condition = {"path": "ticket.skills", "op": "in", "value": ["a", number], "x/~": number}

    check = check_condition(condition)

    assert _get_codes(check) == [
        (("value", 1), "unwritable_number", "authoring"),
        (("x/~",), "unwritable_number", "authoring"),
    ]


def test_reports_a_number_past_a_double():
    check = check_condition({"path": "ticket.posture.overall.score", "op": "lt", "value": 10**400})

    assert _get_codes(check) == [((), "unreadable", "enforcement")]


def test_takes_a_custom_ticket_claims_schema():
    condition = {"path": "ticket.tenant", "op": "eq", "value": "a"}

    assert check_condition(condition, ticket_claims_schema={"tenant": "string"}).problems == ()
    assert _get_codes(check_condition(condition)) == [(("path",), "unknown_path", "authoring")]


def test_reads_the_default_schema_at_call_time(monkeypatch):
    monkeypatch.setattr(datrail_policy, "TICKET_CLAIMS_SCHEMA", {"tenant": "string"})

    assert check_condition({"path": "ticket.tenant", "op": "existing"}).problems == ()


@pytest.mark.parametrize("schema", [{"a": "header"}, {"a": 1}, {"a": {"b": None}}, {"a": str}])
def test_raises_on_a_malformed_schema(schema):
    with pytest.raises(ValueError, match="ticket claims schema"):
        check_condition({"path": "ticket.a", "op": "existing"}, ticket_claims_schema=schema)


def test_raises_on_a_condition_that_is_not_json():
    with pytest.raises(TypeError):
        check_condition({"path": "ticket.a", "op": "in", "value": {1, 2}})


def test_reads_any_mapping_and_sequence():
    condition = MappingProxyType({"path": "ticket.skills", "op": "in", "value": ("a", math.nan)})

    check = check_condition(
        condition, ticket_claims_schema=MappingProxyType(datrail_policy.TICKET_CLAIMS_SCHEMA)
    )

    assert _get_codes(check) == [(("value", 1), "unwritable_number", "authoring")]


def test_stands_in_a_number_for_a_non_finite_operand():
    check = check_condition({"path": "ticket.posture.overall.score", "op": "lt", "value": math.inf})

    assert _get_codes(check) == [(("value",), "unwritable_number", "authoring")]


@pytest.mark.parametrize("depth", [200, 3000])
def test_reports_a_condition_nested_too_deep_for_any_reader(depth):
    deep = 1
    for _ in range(depth):
        deep = [deep]

    check = check_condition({"path": "ticket.skills", "op": "in", "value": deep})

    assert _get_codes(check) == [((), "unreadable", "enforcement")]


def test_reports_a_cycle_as_too_deep():
    cycle = []
    cycle.append(cycle)

    check = check_condition({"path": "ticket.skills", "op": "in", "value": cycle})

    assert _get_codes(check) == [((), "unreadable", "enforcement")]


def test_reports_an_int_past_pythons_digit_limit():
    check = check_condition({"path": "ticket.posture.overall.score", "op": "lt", "value": 10**5000})

    assert _get_codes(check) == [((), "unreadable", "enforcement")]


def test_raises_on_a_key_that_is_not_a_str():
    with pytest.raises(TypeError, match="keys must be str"):
        check_condition({"path": "ticket.a", "op": "existing", True: math.nan})
