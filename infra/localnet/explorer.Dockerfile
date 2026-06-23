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
# published image, so we clone it at a pinned ref and `next start` it. It has
# no `output: standalone`, so the runtime keeps node_modules and runs the
# built `.next` via `pnpm start`.

ARG NODE_VERSION=22

FROM node:${NODE_VERSION}-bookworm-slim AS build
# Pin the explorer source. EXPLORER_REF takes a branch, tag, or full commit
# SHA — bump it to move the explorer version, and Docker's layer cache busts
# only when it changes.
ARG EXPLORER_REPO=https://github.com/solana-foundation/explorer.git
ARG EXPLORER_REF=master
ENV NEXT_TELEMETRY_DISABLED=1
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# corepack ships with Node 22; it resolves the pnpm version pinned in the
# explorer's package.json `packageManager` field.
RUN corepack enable
WORKDIR /app
# Shallow-fetch exactly the pinned ref (works for a branch, tag, or SHA), so
# the clone stays small and reproducible.
RUN git init -q . \
    && git remote add origin "${EXPLORER_REPO}" \
    && git fetch --depth 1 origin "${EXPLORER_REF}" \
    && git checkout -q FETCH_HEAD
RUN pnpm install --frozen-lockfile
RUN pnpm build

FROM node:${NODE_VERSION}-bookworm-slim AS run
ENV NODE_ENV=production
ENV NEXT_TELEMETRY_DISABLED=1
RUN corepack enable
WORKDIR /app
COPY --from=build /app ./
EXPOSE 3000
CMD ["pnpm", "start"]
