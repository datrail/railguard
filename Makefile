.PHONY: build test fmt lint check

build:
	cargo build

test:
	cargo test

fmt:
	cargo fmt

lint:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

check: lint test
