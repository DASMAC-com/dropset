# syntax=docker/dockerfile:1
# Multi-stage build for `dropset-migrate`, the shared database's migration
# runner (docs/data-feeds.md §8), using cargo-chef to cache the dependency
# graph so only first-time and source-changing builds pay the full compile.
#
# Its own image rather than a binary bolted onto the indexer's: the schema
# owner is a separate deploy unit that runs to completion before anything else
# starts — locally as the compose init gate, on AWS before binaries are
# flashed — and must not be versioned with any one consumer.
# `dropset-db-schema` depends on nothing but sqlx and a runtime (no Solana
# tree, no on-chain program), so this is the leanest image in the stack.
#
# Context is the repo root (see docker-compose.yml); the rust image honours the
# workspace `rust-toolchain.toml`. The migrations are embedded at compile time
# (`sqlx::migrate!`), so the runtime image carries only the binary.

FROM rust:1-bookworm AS chef
WORKDIR /app
# The pinned toolchain, in a layer keyed ONLY on `rust-toolchain.toml` and
# ahead of every source COPY: the tag above floats, so rustup has to download
# the pin, and a source-keyed layer re-pays that download on every
# rebuild-after-commit. `rustup toolchain install` takes no argument on
# purpose — the file is the one pin. Every later Rust stage inherits this
# layer, so planner, cook and build all share one compiler.
# Asserted by `.claude/tools/dockerfile_stages.py`; see docs/ci.md §1.
COPY rust-toolchain.toml ./
# One RUN, resolve first: cargo-chef then builds on the pinned compiler too,
# and consolidating keeps hadolint's DL3059 quiet. The order inside the line
# matters as much as between instructions, and the guard checks both.
RUN rustup toolchain install \
    && cargo install cargo-chef --locked

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Cache mounts make a rebuild after a source change incremental. The cargo
# registry and git checkouts persist across builds, and the build step's
# target directory is a persistent cache seeded from the cook stage's target
# (`from=cook`) the first time it is created. So a builder that keeps no
# mount cache, such as a fresh CI runner, starts from the cooked
# dependencies exactly as it did before the mounts, while a local daemon
# rebuilds only the workspace crates that changed. The target id is
# per image and per checkout (the CHECKOUT build arg, which compose fills
# from DS_CHECKOUT, else the caller's directory): cargo judges a workspace
# crate fresh by mtime, so a target shared across worktrees could silently
# ship a sibling's build. A bare `docker build` without the arg falls back
# to one `shared` target. Every cache is `sharing=locked`, serializing
# concurrent builds of one image: cargo's registry lock lives outside the
# registry mount, and the bin copy after `cargo build` runs outside cargo's
# own lock on the target. A mount is not part of any layer, so the binary
# is copied out to /app/bin for the runtime stage. `docker builder prune`
# reclaims the caches. The paths and the target id are ENV rather than ARG
# because an ARG ends with its stage and the builder is a new one; spelled
# inline, a mount overflows 80 columns.
FROM chef AS cook
ENV REG=/usr/local/cargo/registry GIT=/usr/local/cargo/git TGT=/app/target
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,id=migrate-reg,target=$REG,sharing=locked \
    --mount=type=cache,id=migrate-git,target=$GIT,sharing=locked \
    cargo chef cook --release --recipe-path recipe.json --bin dropset-migrate

FROM cook AS builder
ARG CHECKOUT=shared
ENV ID=migrate-tgt-$CHECKOUT
COPY . .
RUN --mount=type=cache,id=migrate-reg,target=$REG,sharing=locked \
    --mount=type=cache,id=migrate-git,target=$GIT,sharing=locked \
    --mount=type=cache,id=$ID,target=$TGT,from=cook,source=$TGT,sharing=locked \
    cargo build --release -p dropset-db-schema --bin dropset-migrate \
    && mkdir -p /app/bin \
    && cp target/release/dropset-migrate /app/bin/

FROM debian:bookworm-slim AS runtime
# ca-certificates only (for Postgres TLS against a managed instance).
# Intentionally unpinned: a thin runtime base where pinning the Debian package
# version would only rot.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/bin/dropset-migrate \
    /usr/local/bin/dropset-migrate
CMD ["dropset-migrate"]
