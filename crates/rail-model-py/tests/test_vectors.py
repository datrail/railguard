from pathlib import Path

import pytest

from datrail_model.conformance import FILES, SHARED, run_vectors

_VECTORS = Path(__file__).parents[3] / "vectors"
_TEXTS = {name: (_VECTORS / name).read_text() for name in (*FILES, *SHARED)}
_OUTCOMES = [(file, outcome) for file in FILES for outcome in run_vectors(file, _TEXTS)]


def test_every_file_has_cases():
    assert {file for file, _ in _OUTCOMES} == set(FILES)


@pytest.mark.parametrize(
    ("file", "outcome"), _OUTCOMES, ids=lambda value: getattr(value, "id", value)
)
def test_vectors(file, outcome):
    assert outcome.matches, (outcome.expect, outcome.got)
