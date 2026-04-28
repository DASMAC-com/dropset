#!/bin/sh
set -e

# Cache key component.
PLATFORM="${RUNNER_OS}-${RUNNER_ARCH}"
echo "PLATFORM=${PLATFORM}" >>"$GITHUB_ENV"

# Solana release bin (solana, cargo-build-sbf) and the SBF scripts dir
# (install.sh, dump.sh).
SOLANA_RELEASE="$HOME/.local/share/solana/install/active_release/bin"
SBPF_TOOLS="$SOLANA_RELEASE/platform-tools-sdk/sbf"
echo "$SOLANA_RELEASE" >>"$GITHUB_PATH"
echo "$SBPF_TOOLS/scripts" >>"$GITHUB_PATH"
