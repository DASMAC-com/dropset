# syntax=docker/dockerfile:1
# cspell:word nologin
# The Dropset node image (`dropset-node`, see docs/node.md). Built by the AWS
# stack's workflow and first run on AWS; it is not part of the localnet
# compose project and has no local make target, since `cargo run -p
# dropset-node` is the local check.
#
# Context is the repo root, so the root `.dockerignore` applies: the build
# needs only the workspace manifests, `rust-toolchain.toml` and the member
# crates' `src/` trees, none of which it excludes (it does drop `**/tests`).
#
# Built for ARM: the workflow builds for `linux/arm64`. Each base below is
# pinned by the digest of its multi-arch index, which carries a
# `linux/arm64/v8` image, so the platform is chosen at build time. The
# compile is native on an arm64 builder and emulated on any other.

FROM rust:1-bookworm@sha256:1b9dc70890ec494e466c87f5ad41ad544bfb30ea6acc99a3e068c47a4ae3f23e AS chef
WORKDIR /app
# The pinned toolchain, in a layer keyed only on `rust-toolchain.toml` and
# ahead of every source COPY, so a source change never re-pays the toolchain
# download. `rustup toolchain install` takes no argument on purpose: the file
# is the one pin. Asserted by `.claude/tools/dockerfile_stages.py`; see
# docs/ci.md §1.
COPY rust-toolchain.toml ./
RUN rustup toolchain install \
    && cargo install cargo-chef --locked

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Dependencies build once, in a layer keyed on the recipe alone, so a source
# change rebuilds only the workspace crates. The registry and git caches use
# this image's own ids, apart from the localnet images' `bots-*` and kin.
FROM chef AS cook
ENV REG=/usr/local/cargo/registry GIT=/usr/local/cargo/git
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,id=node-reg,target=$REG,sharing=locked \
    --mount=type=cache,id=node-git,target=$GIT,sharing=locked \
    cargo chef cook --release --locked --recipe-path recipe.json \
    --bin dropset-node

# BUILD_COMMIT is the full git commit SHA the image is built from. The binary
# bakes it into its start line at compile time; left empty, the line reads
# `unknown`. The source is COPYed rather than bind-mounted over a persistent
# target cache, as the localnet images do: only CI builds this image, from a
# fresh target each time, so there is no cached target to go stale by mtime.
FROM cook AS builder
ARG BUILD_COMMIT=
COPY . .
RUN --mount=type=cache,id=node-reg,target=$REG,sharing=locked \
    --mount=type=cache,id=node-git,target=$GIT,sharing=locked \
    DROPSET_NODE_BUILD="$BUILD_COMMIT" \
    cargo build --release --locked --bin dropset-node

FROM debian:bookworm-slim@sha256:e67eeead736ff7bbe258ef088bff683635c561b1fcfefad0806c1df4ab49ce0e AS runtime
# ca-certificates only, for shell tooling run in the image. Intentionally
# unpinned: a thin runtime base where pinning the Debian package version would
# only rot. The user is created in the same layer, with a fixed numeric id so
# a host can verify the image runs as non-root without resolving a name.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home \
    --shell /usr/sbin/nologin dropset
COPY --from=builder /app/target/release/dropset-node \
    /usr/local/bin/dropset-node
USER 10001
# Exec form, so the node is PID 1 and receives the host's stop signal itself.
# It handles the terminate signal, prints its shutdown line and exits 0.
CMD ["dropset-node"]
