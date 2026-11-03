import json
from pathlib import Path

import pytest

from datrail_policy import BundleCheck, validate_bundle

_VECTORS = Path(__file__).parents[3] / "vectors" / "parse_bundle_conformance_cases.json"
_CASES = json.loads(_VECTORS.read_text())["cases"]


def test_has_cases():
    assert _CASES


@pytest.mark.parametrize("case", _CASES, ids=lambda case: case["name"])
def test_vectors(case):
    check = validate_bundle(json.dumps(case["bundle"]))

    assert check.usable is case["usable"], check.problem


def test_refusal_says_why():
    assert validate_bundle("{}") == BundleCheck(
        usable=False,
        problem="unusable policy bundle: missing field `schema_version` at line 1 column 2",
    )
