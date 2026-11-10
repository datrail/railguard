# docker build -f crates/rail-envoy/Dockerfile .
# Pinned by digest: a tag is mutable.
FROM rust:1.99.1-slim-bookworm@sha256:ed2c95ca377d6571b124b05e75e01207973ded639dce7d5542eb7bcd797e32aa AS build
# The image's own toolchain, the one rust-toolchain.toml names, without its clippy and rustfmt.
ENV RUSTUP_TOOLCHAIN=1.99.1
WORKDIR /src
# The socket's directory, made here since the runtime image has no shell.
RUN install -d -m 0750 /out/run/rail
COPY . .
# Locked: two builds at once, as a compose file building it per service, would unpack a crate
# into the shared cache together.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked -p rail-envoy && cp target/release/rail-envoy /out/

FROM gcr.io/distroless/cc-debian12@sha256:e5d81ddde149641e2a9ba55be4545bc125c67de07508b03ba4c22e6eb0ded5aa AS railguard-envoy
COPY LICENSE /
# One name for every host's binary, so probes and commands outlive which binary it is.
COPY --from=build /out/rail-envoy /usr/local/bin/rail
# Envoy reaches the socket from its own container, as a member of group 10001.
COPY --from=build --chown=10001:10001 /out/run /run
USER 10001:10001
VOLUME /run/rail
CMD ["/usr/local/bin/rail"]
