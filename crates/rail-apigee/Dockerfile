# docker build -f crates/rail-apigee/Dockerfile .
# Pinned by digest: a tag is mutable.
FROM rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7 AS build
# The image's own toolchain, the one rust-toolchain.toml names, without its clippy and rustfmt.
ENV RUSTUP_TOOLCHAIN=1.99.1
WORKDIR /src
# Records the crates built in, so the image's SBOM lists them. Before the sources, so it's cached.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo install --locked cargo-auditable@0.7.7
COPY . .
# The startup line's release and commit, set by the release workflow (unset: `dev` and
# `unknown`); here, so a change rebuilds only from this step.
ARG RAIL_VERSION
ARG RAIL_REVISION
# Locked: two builds at once, as a compose file building it per service, would unpack a crate
# into the shared cache together.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo auditable build --release --locked -p rail-apigee && install -D target/release/rail-apigee /out/rail-apigee

FROM gcr.io/distroless/cc-debian13@sha256:e1f5543d45a2cd7c02a5b43dda5f0ff30729b4832113759d01ed39b0b152cddb AS railguard-apigee
COPY LICENSE crates/rail-apigee/THIRD-PARTY-NOTICES /
# One name for every host's binary, so probes and commands outlive which binary it is.
COPY --from=build /out/rail-apigee /usr/local/bin/rail
# Not root: the callout receives every request header, credentials included.
USER 10001:10001
# RAIL_CALLOUT_PORT's default, and Cloud Run's.
EXPOSE 8080
# No default role: a deployment names the one it runs, e.g. `gateway`.
ENTRYPOINT ["/usr/local/bin/rail"]
