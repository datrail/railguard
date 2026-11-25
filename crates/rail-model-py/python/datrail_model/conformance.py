"""The conformance vectors' runner, as the Rust tests run them; it reads only the texts it's given."""

import json
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

from datrail_model import _native

__all__ = ["FILES", "SHARED", "VectorOutcome", "run_vectors"]

FILES: tuple[str, ...] = tuple(_native.FILES)
SHARED: tuple[str, ...] = tuple(_native.SHARED)


@dataclass(frozen=True)
class VectorOutcome:
    id: str
    expect: Any
    got: Any
    # As Rust compares them, so `1` and `1.0`, or `true` and `1`, differ.
    matches: bool


def run_vectors(file: str, texts: Mapping[str, str]) -> list[VectorOutcome]:
    """Every case of `file`, one of `FILES`, run by its layer; `texts` holds, by path under
    `vectors/`, `file` and each of `SHARED`."""
    return [
        VectorOutcome(id, json.loads(expect), json.loads(got), matches)
        for id, expect, got, matches in _native.run_vectors(file, dict(texts))
    ]
