.PHONY: build test test-py fmt fmt-py fmt-md lint lint-py lint-md check coverage proto notices e2e-envoy-images e2e-envoy e2e-apigee-images e2e-apigee e2e-down

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

# Pinned, so every run formats the same; needs npx.
PRETTIER = npx --yes prettier@3.6.2

fmt-md:
	$(PRETTIER) --write "**/*.md"

lint:
	cargo fmt --check
	cargo clippy --locked --all-targets -- -D warnings
	cargo clippy --locked -p rail-policy-py -- -D warnings

lint-py:
	$(MAKE) -C crates/rail-policy-py lint

lint-md:
	$(PRETTIER) --check "**/*.md"

check: lint lint-py test test-py

# Needs `cargo install cargo-llvm-cov` and `rustup component add llvm-tools`.
coverage:
	cargo llvm-cov --html
	cargo llvm-cov report
	@echo "Report: target/llvm-cov/html/index.html"

# Regenerates apigee-callout-proto's committed code, after its .proto changes.
proto:
	cargo run --locked --manifest-path crates/apigee-callout-proto/codegen/Cargo.toml --target-dir target/codegen

# Each image's third-party licences, from its dependencies; needs cargo-about.
notices:
	for host in apigee envoy; do \
		cargo about generate --frozen --fail -m crates/rail-$$host/Cargo.toml -c .notices/about.toml -o crates/rail-$$host/THIRD-PARTY-NOTICES .notices/about.hbs || exit 1; \
	done

# The stack e2e-down removes.
STACK = envoy

# Builds the stack's images and pulls the others, so the run finds them ready.
e2e-envoy-images:
	docker compose -f e2e/envoy/compose.yml build
	docker compose -f e2e/envoy/compose.yml pull --ignore-buildable

# The result is the driver's exit code.
e2e-envoy:
	docker compose -f e2e/envoy/compose.yml up --build --force-recreate --abort-on-container-exit --exit-code-from driver

e2e-apigee-images:
	docker compose -f e2e/apigee/compose.yml build
	docker compose -f e2e/apigee/compose.yml pull --ignore-buildable

e2e-apigee:
	docker compose -f e2e/apigee/compose.yml up --build --force-recreate --abort-on-container-exit --exit-code-from driver

e2e-down:
	docker compose -f e2e/$(STACK)/compose.yml down --volumes --remove-orphans
