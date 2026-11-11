# rail-policy-py

The rules language as a Python package, `datrail-policy`, for Rail Center, to check each rule before publishing it, and
for RailDash, to try a rule on a sample call. Both then run the very code the gateways enforce rules with, instead of a
Python copy that could drift from it.

It's the Python binding of `rail-policy`, built with maturin into the `datrail-policy` wheel. It exposes:

- `validate_bundle`, which says whether a policy bundle is usable;
- `check_condition`, which reports every problem with a condition, and whether every reader refuses it or only Rail
  Center;
- `compile_bundle` and `judge_sample`, which judge a sample call against a bundle and trace the policies evaluated;
- `read_ticket`, which reads an `x-rail` value into the claims `judge_sample` takes.

`make test-py` builds the wheel and runs its tests.
