from dataclasses import dataclass

from datrail_policy import _native


@dataclass(frozen=True)
class BundleCheck:
    usable: bool
    problem: str | None


def validate_bundle(json: str) -> BundleCheck:
    problem = _native.validate_bundle(json)
    return BundleCheck(usable=problem is None, problem=problem)
