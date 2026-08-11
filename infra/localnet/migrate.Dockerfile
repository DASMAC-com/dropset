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
RUN cargo install cargo-chef --locked
WORKDIR /app

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
