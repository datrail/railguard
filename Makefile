.PHONY: build test test-py fmt fmt-py lint lint-py check coverage

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
