# Multi-stage build for the collector binary (`fx-survey-coinbase`), using
# cargo-chef to cache the dependency graph so only first-time and
# source-changing builds pay the full compile. The app depends on
# `dropset-feeds` with only its `http` + `store` features — not the Solana
# `rpc` tree and not the on-chain program — so this build stays lean and never
# pulls the anchor-next git source.
#
# There is no migrate binary here any more: schema provisioning belongs to
# `dropset-migrate` (infra/localnet/migrate.Dockerfile), the single schema
# owner (docs/data-feeds.md §8). This image only ever asserts the schema.
#
# Context is the repo root (see docker-compose.yml); the rust image honours the
# workspace `rust-toolchain.toml`. The insert SQL is embedded at compile time
# (`include_str!`), so the runtime image carries only the binary.

FROM rust:1-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json \
    --bin fx-survey-coinbase
COPY . .
RUN cargo build --release -p dropset-fx-survey --bin fx-survey-coinbase

FROM debian:bookworm-slim AS runtime
# ca-certificates only (for the Coinbase HTTPS + Postgres TLS). Intentionally
# unpinned: a thin runtime base where pinning the Debian package version would
# only rot.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/fx-survey-coinbase \
    /usr/local/bin/fx-survey-coinbase
CMD ["fx-survey-coinbase"]
