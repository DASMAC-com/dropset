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

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json --bin dropset-migrate
COPY . .
RUN cargo build --release -p dropset-db-schema --bin dropset-migrate

FROM debian:bookworm-slim AS runtime
# ca-certificates only (for Postgres TLS against a managed instance).
# Intentionally unpinned: a thin runtime base where pinning the Debian package
# version would only rot.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/dropset-migrate \
    /usr/local/bin/dropset-migrate
CMD ["dropset-migrate"]
