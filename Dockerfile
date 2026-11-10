# docker build --target railguard-envoy .   (or railguard-apigee)
# Pinned by digest: a tag is mutable.
FROM rust:1.99.1-slim-bookworm@sha256:ed2c95ca377d6571b124b05e75e01207973ded639dce7d5542eb7bcd797e32aa AS source
# The image's own toolchain, the one rust-toolchain.toml names, without its clippy and rustfmt.
ENV RUSTUP_TOOLCHAIN=1.99.1
WORKDIR /src
COPY . .

FROM source AS build-envoy
# The socket's directory, made here since the runtime image has no shell.
RUN install -d -m 0750 /out/run/rail
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p rail-envoy && cp target/release/rail-envoy /out/

FROM source AS build-apigee
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p rail-apigee && install -D target/release/rail-apigee /out/rail-apigee

FROM gcr.io/distroless/cc-debian12@sha256:e5d81ddde149641e2a9ba55be4545bc125c67de07508b03ba4c22e6eb0ded5aa AS railguard-envoy
COPY LICENSE /
COPY --from=build-envoy /out/rail-envoy /rail-envoy
# Envoy reaches the socket from its own container, as a member of group 10001.
COPY --from=build-envoy --chown=10001:10001 /out/run /run
USER 10001:10001
VOLUME /run/rail
CMD ["/rail-envoy"]

FROM gcr.io/distroless/cc-debian12@sha256:e5d81ddde149641e2a9ba55be4545bc125c67de07508b03ba4c22e6eb0ded5aa AS railguard-apigee
COPY LICENSE /
COPY --from=build-apigee /out/rail-apigee /rail-apigee
# Not root: the callout receives every request header, credentials included.
USER 10001:10001
# RAIL_GATEWAY_PORT's default, and Cloud Run's.
EXPOSE 8080
CMD ["/rail-apigee"]
