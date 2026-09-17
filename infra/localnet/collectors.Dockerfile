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
# Context is the repo root (see docker-compose.yml). The committed
# `rust-toolchain.toml` is what pins the compiler — authoritative for CI,
# local builds and this image alike — and the `rust:1-bookworm` tag is only
# the rustup bootstrap that resolves it. An earlier revision of this comment
# claimed the opposite (that the workspace commits no toolchain file, so the
# tag was the only pin); that has been false since the 2026-08-24 pin commit,
# and it misdirected the diagnosis of exactly the stall the chef stage below
# now prevents. The insert SQL is embedded at compile time (`include_str!`), so
# the runtime image carries only the binaries.

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
    /app/target/release/market-data-erapi \
    /app/target/release/market-data-frankfurter \
    /app/target/release/market-data-kraken \
    /app/target/release/market-data-pyth \
    /usr/local/bin/
# The fair-value estimator, which is neither: it READS both tables and
# publishes `fair_price`. Its own COPY rather than a line in either group
# above, so the two comments stay true — grouping it with the tick collectors
# would make this image claim it writes `spot_ticks`.
COPY --from=builder /app/target/release/market-data-estimator \
    /usr/local/bin/
# The keyless reference feed is the default; every other service overrides it
# with its own `command:`.
CMD ["market-data-coinbase"]
