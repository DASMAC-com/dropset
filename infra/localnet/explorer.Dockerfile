# syntax=docker/dockerfile:1
# cspell:word corepack
# cspell:word bookworm

# Solana Explorer, built from source and served over http://localhost.
#
# Why build our own image: the hosted explorer.solana.com is a *public*
# origin, and modern browsers block a public page from reaching a *loopback*
# RPC (Brave by default, Safari always, others under Private Network Access),
# so it stalls on "loading" against the localnet. Serving the explorer from
# http://localhost makes the page itself loopback, so its client-side fetch
# to the loopback validator is loopback -> loopback and no browser blocks it.
#
# The explorer (solana-foundation/explorer) ships no Dockerfile and no
# published image, so we clone it at a configurable ref (default `master`;
# override EXPLORER_REF with a tag or commit SHA to pin) and `next start` it.
# It has no `output: standalone`, so the runtime keeps node_modules and runs
# the built `.next` via `pnpm start`.

ARG NODE_VERSION=22

FROM node:${NODE_VERSION}-bookworm-slim AS build
# Pin the explorer source. EXPLORER_REF takes a branch, tag, or full commit
# SHA — bump it to move the explorer version, and Docker's layer cache busts
# only when it changes.
ARG EXPLORER_REPO=https://github.com/solana-foundation/explorer.git
ARG EXPLORER_REF=master
ENV NEXT_TELEMETRY_DISABLED=1
# Install git + ca-certificates (to clone) and enable corepack, which ships
# with Node 22 and resolves the pnpm version pinned in the explorer's
# package.json `packageManager` field. The Debian packages are intentionally
# unpinned: this is a from-source dev image, and pinning their versions would
# only rot.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && corepack enable
WORKDIR /app
# Shallow-fetch exactly the pinned ref (works for a branch, tag, or SHA), then
# install and build — one layer, re-run only when EXPLORER_REF changes.
RUN git init -q . \
    && git remote add origin "${EXPLORER_REPO}" \
    && git fetch --depth 1 origin "${EXPLORER_REF}" \
    && git checkout -q FETCH_HEAD \
    && pnpm install --frozen-lockfile \
    && pnpm build

FROM node:${NODE_VERSION}-bookworm-slim AS run
ENV NODE_ENV=production
ENV NEXT_TELEMETRY_DISABLED=1
# Share corepack's cache from a world-readable location so the unprivileged
# node user (set below) resolves pnpm without a writable home cache.
ENV COREPACK_HOME=/opt/corepack
WORKDIR /app
# --chown so the node user owns node_modules / .next and can write the runtime
# .next cache; the build stage runs as root and would otherwise leave them
# root-owned and unwritable.
COPY --from=build --chown=node:node /app ./
# Enable corepack and pre-install the pnpm version pinned in the explorer's
# package.json into the shared COREPACK_HOME, so `pnpm start` needs no runtime
# download or per-user cache. chmod makes the cache readable by the node user.
RUN corepack enable \
    && corepack install \
    && chmod -R a+rX /opt/corepack
# Drop root: the explorer is just a static Next server, so it never needs it.
USER node
EXPOSE 3000
CMD ["pnpm", "start"]
