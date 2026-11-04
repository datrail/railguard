# rail-policy-py

The Python binding of `rail-policy`, built with maturin into the `datrail-policy` wheel. It exposes `validate_bundle`, which says whether a policy bundle is usable; `compile_bundle` and `judge_sample`, which judge a sample call against a bundle and trace the policies evaluated; and `read_ticket`, which reads an `x-rail` value into the claims `judge_sample` takes. `make test-py` builds the wheel and runs its tests.
