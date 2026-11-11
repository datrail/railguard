# docker build -f crates/rail-envoy/Dockerfile .
# Pinned by digest: a tag is mutable.
FROM rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7 AS build
# The image's own toolchain, the one rust-toolchain.toml names, without its clippy and rustfmt.
ENV RUSTUP_TOOLCHAIN=1.99.1
WORKDIR /src
# Records the crates built in, so the image's SBOM lists them. Before the sources, so it's cached.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo install --locked cargo-auditable@0.7.7
# The socket's directory, made here since the runtime image has no shell.
RUN install -d -m 0750 /out/run/rail
COPY . .
# The startup line's release and commit, set by the release workflow (unset: `dev` and
# `unknown`); here, so a change rebuilds only from this step.
ARG RAIL_VERSION
ARG RAIL_REVISION
# Locked: two builds at once, as a compose file building it per service, would unpack a crate
# into the shared cache together.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo auditable build --release --locked -p rail-envoy && cp target/release/rail-envoy /out/

FROM gcr.io/distroless/cc-debian13@sha256:e1f5543d45a2cd7c02a5b43dda5f0ff30729b4832113759d01ed39b0b152cddb AS railguard-envoy
COPY LICENSE crates/rail-envoy/THIRD-PARTY-NOTICES /
# One name for every host's binary, so probes and commands outlive which binary it is.
COPY --from=build /out/rail-envoy /usr/local/bin/rail
# Envoy reaches the socket from its own container, as a member of group 10001.
COPY --from=build --chown=10001:10001 /out/run /run
USER 10001:10001
VOLUME /run/rail
CMD ["/usr/local/bin/rail"]
