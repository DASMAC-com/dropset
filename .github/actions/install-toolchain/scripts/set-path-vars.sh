#!/bin/sh
set -e

# The toolchain cache key, built here rather than inline in the action so the
# restore and save steps read one definition: an entry saved under a key the
# next run doesn't look up is a silent, permanent cache miss.
PLATFORM="${RUNNER_OS}-${RUNNER_ARCH}"
KEY="toolchain-solana-${SOLANA_VERSION}-anchor-${ANCHOR_VERSION}"
KEY="${KEY}-platform-tools-${PLATFORM_TOOLS_VERSION}"
echo "TOOLCHAIN_CACHE_KEY=${KEY}-${PLATFORM}" >>"$GITHUB_ENV"

# Solana release bin (solana, cargo-build-sbf).
SOLANA_RELEASE="$HOME/.local/share/solana/install/active_release/bin"
echo "$SOLANA_RELEASE" >>"$GITHUB_PATH"
