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
RUN cargo install cargo-chef --locked
WORKDIR /app

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
COPY --from=builder /app/target/release/dropset-maker-bot /usr/local/bin/dropset-maker-bot
COPY --from=builder /app/target/release/dropset-taker-bot /usr/local/bin/dropset-taker-bot
# The bots resolve their keypairs relative to the working dir (`keys/…`); the
# compose services bind-mount the repo `keys/` here. Each service overrides
# `command` with its bot binary + the host-validator `--rpc`.
WORKDIR /app
CMD ["dropset-maker-bot"]
