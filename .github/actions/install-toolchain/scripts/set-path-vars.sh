#!/bin/sh
set -e

# The toolchain cache key, built here rather than inline in the action so the
# restore and save steps read one definition: an entry saved under a key the
# next run doesn't look up is a silent, permanent cache miss.
PLATFORM="${RUNNER_OS}-${RUNNER_ARCH}"
KEY="toolchain-solana-${SOLANA_VERSION}-anchor-${ANCHOR_REVISION}"
echo "TOOLCHAIN_CACHE_KEY=${KEY}-${PLATFORM}" >>"$GITHUB_ENV"

# Solana release bin (solana, cargo-build-sbf) and the SBF scripts dir
# (install.sh, dump.sh).
SOLANA_RELEASE="$HOME/.local/share/solana/install/active_release/bin"
SBPF_TOOLS="$SOLANA_RELEASE/platform-tools-sdk/sbf"
echo "$SOLANA_RELEASE" >>"$GITHUB_PATH"
echo "$SBPF_TOOLS/scripts" >>"$GITHUB_PATH"
