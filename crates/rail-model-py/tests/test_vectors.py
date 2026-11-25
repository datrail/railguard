from pathlib import Path

import pytest

from datrail_model.conformance import run_vectors

_VECTORS = Path(__file__).parents[3] / "vectors"
_FILES = [
    "request_line.json",
    "mcp.json",
    "condition/path.json",
    "condition/condition.json",
    "condition/op.json",
    "capture/entry.json",
    "capture/record.json",
    "capture/matching.json",
    "capture/window.json",
    "capture/windowing.json",
]
_OUTCOMES = [(file, outcome) for file in _FILES for outcome in run_vectors(_VECTORS, file)]


def test_every_file_has_cases():
    assert {file for file, _ in _OUTCOMES} == set(_FILES)


@pytest.mark.parametrize(
    ("file", "outcome"), _OUTCOMES, ids=lambda value: getattr(value, "id", value)
)
def test_vectors(file, outcome):
    assert outcome.got == outcome.expect
