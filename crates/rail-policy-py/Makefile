.PHONY: test fmt lint

UV_RUN := uv run --locked

# Rebuilds the wheel, which uv would otherwise reuse from its cache.
test:
	$(UV_RUN) --reinstall-package datrail-policy pytest

# --only-dev, so ruff runs without building the wheel.
fmt:
	$(UV_RUN) --only-dev ruff check --fix .
	$(UV_RUN) --only-dev ruff format .

lint:
	$(UV_RUN) --only-dev ruff format --check .
	$(UV_RUN) --only-dev ruff check .
