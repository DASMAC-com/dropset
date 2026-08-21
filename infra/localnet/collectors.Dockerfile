# Multi-stage build for the collector binaries, using cargo-chef to cache the
# dependency graph so only first-time and source-changing builds pay the full
# compile. The app depends on `dropset-feeds` with only its `http` + `store`
# features — not the Solana `rpc` tree and not the on-chain program — so
# this build stays lean and never pulls the anchor-next git source.
#
# **Every** binary in the package is built and shipped, not just one, because
# the compose file runs all of them from this one image and selects between them
# with `command:`. Naming a single bin here is what made three FX services fail
# at start with `executable file not found in $PATH` — the image built
# fine, so nothing caught it until a container ran.
#
# The build step needs no edit for a new collector (building the package builds
# its bins), but the runtime COPY below does: it enumerates them rather than
# globbing, because `market-data-*` in `target/release/` would also match
# cargo's `.d` dependency files. A bin added to `market-data/Cargo.toml` without
# a line there reproduces exactly the failure above, so add both together.
#
# There is no migrate binary here: schema provisioning belongs to
# `dropset-migrate` (migrate.Dockerfile), the single schema owner
# (docs/data-feeds.md §8). This image only ever asserts the schema.
#
# Context is the repo root (see docker-compose.yml). The `rust:1-bookworm` tag
# is what pins the compiler: the workspace commits no `rust-toolchain.toml`, so
# there is nothing in-tree for the image to honour and the tag is the only pin.
# The insert SQL is embedded at compile time (`include_str!`), so the runtime
# image carries only the binaries.

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
# ca-certificates only (every collector's HTTPS venue + Postgres TLS).
# Intentionally unpinned: a thin runtime base where pinning the Debian package
# version would only rot.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# The candle collectors, writing `cex_prices`...
COPY --from=builder /app/target/release/market-data-alphavantage \
    /app/target/release/market-data-coinbase \
    /app/target/release/market-data-oanda \
    /app/target/release/market-data-twelvedata \
    /usr/local/bin/
# ...and the tick collectors, writing `spot_ticks`.
COPY --from=builder /app/target/release/market-data-coinbase-ticker \
    /app/target/release/market-data-kraken \
    /app/target/release/market-data-pyth \
    /usr/local/bin/
# The keyless reference feed is the default; every other service overrides it
# with its own `command:`.
CMD ["market-data-coinbase"]
