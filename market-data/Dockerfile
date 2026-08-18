# Multi-stage build for the collector binaries, using cargo-chef to cache the
# dependency graph so only first-time and source-changing builds pay the full
# compile. The app depends on `dropset-feeds` with only its `http` + `store`
# features — not the Solana `rpc` tree and not the on-chain program — so
# this build stays lean and never pulls the anchor-next git source.
#
# **Every** binary in the package is built and shipped, not just the Coinbase
# one, because the compose file runs all four services from this one image and
# selects between them with `command:`. Naming a single bin here is what made
# the three FX services fail at start with `executable file not found in
# $PATH` — the image built fine, so nothing caught it until a container ran.
# Building the package builds its bins, so a fifth collector needs no edit.
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
    -p dropset-market-data
COPY . .
RUN cargo build --release -p dropset-market-data

FROM debian:bookworm-slim AS runtime
# ca-certificates only (for the Coinbase HTTPS + Postgres TLS). Intentionally
# unpinned: a thin runtime base where pinning the Debian package version would
# only rot.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/market-data-alphavantage \
    /app/target/release/market-data-coinbase \
    /app/target/release/market-data-oanda \
    /app/target/release/market-data-twelvedata \
    /usr/local/bin/
# The keyless reference feed is the default; the FX services each override it
# with their own `command:`.
CMD ["market-data-coinbase"]
