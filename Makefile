.PHONY: build test test-py fmt lint check coverage

build:
	cargo build --locked

test:
	cargo test --locked

# Rebuilds the wheel, which uv would otherwise reuse from its cache.
test-py:
	cd crates/rail-policy-py && uv run --locked --reinstall-package datrail-policy pytest

fmt:
	cargo fmt

lint:
	cargo fmt --check
	cargo clippy --locked --all-targets -- -D warnings
	cargo clippy --locked -p rail-policy-py -- -D warnings

check: lint test test-py

# Needs `cargo install cargo-llvm-cov` and `rustup component add llvm-tools`.
coverage:
	cargo llvm-cov --html
	cargo llvm-cov report
	@echo "Report: target/llvm-cov/html/index.html"
