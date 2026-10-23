# syntax=docker/dockerfile:1
# Multi-stage build for the collector binaries, using cargo-chef to cache the
# dependency graph so only first-time and source-changing builds pay the full
# compile. The app depends on `dropset-feeds` with its `http`, `store` and
# `orca` features. `orca` brings in the Solana client and Orca's SDK (crates.io
# only), but not the indexer's `rpc` walk and not the on-chain program, so
# this build never pulls the anchor-next git source.
#
# **Every** binary in the package is built and shipped, not just one, because
# the compose file runs all of them from this one image and selects between them
# with `command:`. Naming a single bin here is what made three FX services fail
# at start with `executable file not found in $PATH` — the image built
# fine, so nothing caught it until a container ran.
#
# The build step needs no edit for a new collector (building the package builds
# its bins, and every executable `market-data-*` is copied out of the target
# cache), but the runtime COPY below does: it enumerates them rather than
# globbing, so the image's contents are stated in this file. A bin added to
# `market-data/Cargo.toml` without a line there reproduces exactly the failure
# above, so add both together.
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

# Cache mounts make a rebuild after a source change incremental. The cargo
# registry and git checkouts persist across builds, and the build step's
# target directory is a persistent cache seeded from the cook stage's target
# (`from=cook`) the first time it is created. So a builder that keeps no
# mount cache, such as a fresh CI runner, starts from the cooked
# dependencies exactly as it did before the mounts, while a local daemon
# rebuilds only the workspace crates that changed. The target id is
# per image and per checkout (the CHECKOUT build arg, which compose fills
# from DS_CHECKOUT): cargo judges a workspace crate fresh by mtime, so a
# target shared across worktrees could silently ship a sibling's build. A
# bare `docker build` without the arg falls back to one `shared` target.
# `sharing=locked` serializes
# concurrent builds of one image over the registry and git caches, whose
# cargo lock lives outside the mount; the target needs no such flag, since
# cargo locks its own build directory. A mount is not part of any layer, so
# the binaries are copied out to /app/bin for the runtime stage. `docker
# builder prune` reclaims the caches. The paths are ENV rather than ARG
# because an ARG ends with its stage and the builder is a new one; spelled
# inline, a mount overflows 80 columns.
FROM chef AS cook
ENV REG=/usr/local/cargo/registry GIT=/usr/local/cargo/git TGT=/app/target
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,id=collectors-reg,target=$REG,sharing=locked \
    --mount=type=cache,id=collectors-git,target=$GIT,sharing=locked \
    cargo chef cook --release --recipe-path recipe.json \
    -p dropset-market-data

FROM cook AS builder
ARG CHECKOUT=shared
ENV TGT_ID=collectors-tgt-$CHECKOUT
COPY . .
# The `rm` drops last build's bins from the cache, so the `find` below copies
# only what this build produced and a runtime COPY naming a removed bin fails.
RUN --mount=type=cache,id=collectors-reg,target=$REG,sharing=locked \
    --mount=type=cache,id=collectors-git,target=$GIT,sharing=locked \
    --mount=type=cache,id=$TGT_ID,target=$TGT,from=cook,source=$TGT \
    rm -f target/release/market-data-* \
    && cargo build --release -p dropset-market-data \
    && mkdir -p /app/bin \
    && find target/release -maxdepth 1 -type f -perm -u+x \
    -name 'market-data-*' -exec cp {} /app/bin/ \;

FROM debian:bookworm-slim AS runtime
# ca-certificates only (every collector's HTTPS venue + Postgres TLS).
# Intentionally unpinned: a thin runtime base where pinning the Debian package
# version would only rot.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# The candle collectors, writing `cex_prices`...
COPY --from=builder /app/bin/market-data-alphavantage \
    /app/bin/market-data-coinbase \
    /app/bin/market-data-oanda \
    /app/bin/market-data-twelvedata \
    /usr/local/bin/
# ...and the tick collectors, writing `spot_ticks`.
COPY --from=builder /app/bin/market-data-coinbase-ticker \
    /app/bin/market-data-erapi \
    /app/bin/market-data-frankfurter \
    /app/bin/market-data-kraken \
    /app/bin/market-data-orca \
    /app/bin/market-data-pyth \
    /usr/local/bin/
# The fair-value estimator, which is neither: it READS both tables and
# publishes `fair_price`. Its own COPY rather than a line in either group
# above, so the two comments stay true — grouping it with the tick collectors
# would make this image claim it writes `spot_ticks`.
COPY --from=builder /app/bin/market-data-estimator \
    /usr/local/bin/
# The keyless reference feed is the default; every other service overrides it
# with its own `command:`.
CMD ["market-data-coinbase"]
