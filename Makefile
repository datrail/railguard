.PHONY: build test test-py fmt fmt-py lint lint-py check coverage e2e-envoy-images e2e-envoy e2e-down

build:
	cargo build --locked

test:
	cargo test --locked

test-py:
	$(MAKE) -C crates/rail-policy-py test

fmt:
	cargo fmt

fmt-py:
	$(MAKE) -C crates/rail-policy-py fmt

lint:
	cargo fmt --check
	cargo clippy --locked --all-targets -- -D warnings
	cargo clippy --locked -p rail-policy-py -- -D warnings

lint-py:
	$(MAKE) -C crates/rail-policy-py lint

check: lint lint-py test test-py

# Needs `cargo install cargo-llvm-cov` and `rustup component add llvm-tools`.
coverage:
	cargo llvm-cov --html
	cargo llvm-cov report
	@echo "Report: target/llvm-cov/html/index.html"

# The stack e2e-down removes.
STACK = envoy

# Builds the stack's images and pulls the others, so the run finds them ready.
e2e-envoy-images:
	docker compose -f e2e/envoy/compose.yml build
	docker compose -f e2e/envoy/compose.yml pull --ignore-buildable

# The result is the driver's exit code.
e2e-envoy:
	docker compose -f e2e/envoy/compose.yml up --build --force-recreate --abort-on-container-exit --exit-code-from driver

e2e-down:
	docker compose -f e2e/$(STACK)/compose.yml down --volumes --remove-orphans
