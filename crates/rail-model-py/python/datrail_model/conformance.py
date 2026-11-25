"""The conformance vectors' runner, as the Rust tests run them."""

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from datrail_model import _native

__all__ = ["VectorOutcome", "run_vectors"]


@dataclass(frozen=True)
class VectorOutcome:
    id: str
    expect: Any
    got: Any


def run_vectors(vectors: Path, file: str) -> list[VectorOutcome]:
    """Every case of `file`, a path under `vectors`, run by its layer."""
    return [
        VectorOutcome(id, json.loads(expect), json.loads(got))
        for id, expect, got in _native.run_vectors(str(vectors), file)
    ]
