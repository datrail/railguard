# docker build -f crates/rail-apigee/Dockerfile .
# Pinned by digest: a tag is mutable.
FROM rust:1.99.1-slim-bookworm@sha256:ed2c95ca377d6571b124b05e75e01207973ded639dce7d5542eb7bcd797e32aa AS build
# The image's own toolchain, the one rust-toolchain.toml names, without its clippy and rustfmt.
ENV RUSTUP_TOOLCHAIN=1.99.1
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p rail-apigee && install -D target/release/rail-apigee /out/rail-apigee

FROM gcr.io/distroless/cc-debian12@sha256:e5d81ddde149641e2a9ba55be4545bc125c67de07508b03ba4c22e6eb0ded5aa AS railguard-apigee
COPY LICENSE /
COPY --from=build /out/rail-apigee /rail-apigee
# Not root: the callout receives every request header, credentials included.
USER 10001:10001
# RAIL_GATEWAY_PORT's default, and Cloud Run's.
EXPOSE 8080
CMD ["/rail-apigee"]
