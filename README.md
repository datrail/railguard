# RailGuard

The Rust workspace for DatRail's rules language and enforcement layer.

```sh
make lint
make test
make test-py
```

`make check` runs all three, as CI does. `crates/rail-policy-py` builds the `datrail-policy` wheel. `make test-py` needs [uv](https://docs.astral.sh/uv/).
