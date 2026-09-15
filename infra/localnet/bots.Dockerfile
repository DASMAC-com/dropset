# Multi-stage build for the Dropset localnet bots (`dropset-maker-bot` +
# `dropset-taker-bot`), using cargo-chef to cache the dependency graph so only
# first-time and source-changing builds pay the full compile. Both are host
# builds over `dropset-sdk` (the `fetch` RpcClient + quoting helpers) and
# solana 3.x — not the on-chain program — though the maker also pulls
# anchor-lang-v2 (host, default features) for the FillEvent self-CPI tag.
#
# Context is the repo root (see docker-compose.yml); the rust image honours the
# workspace `rust-toolchain.toml`. The bots are localnet-only: they sign with
# the mounted `keys/` keypairs (and the taker the mounted mock-mint authority)
# and guard on the genesis hash, refusing any public cluster.

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
    --bin dropset-maker-bot --bin dropset-taker-bot
COPY . .
RUN cargo build --release \
    --bin dropset-maker-bot --bin dropset-taker-bot

FROM debian:bookworm-slim AS runtime
# ca-certificates for the price-feed / RPC TLS. Intentionally unpinned: a thin
# runtime base where pinning the Debian package version would only rot.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/dropset-maker-bot \
    /usr/local/bin/dropset-maker-bot
COPY --from=builder /app/target/release/dropset-taker-bot \
    /usr/local/bin/dropset-taker-bot
# The bots resolve their keypairs relative to the working dir (`keys/…`); the
# compose services bind-mount the repo `keys/` here. Each service overrides
# `command` with its bot binary + the host-validator `--rpc`.
WORKDIR /app
CMD ["dropset-maker-bot"]
