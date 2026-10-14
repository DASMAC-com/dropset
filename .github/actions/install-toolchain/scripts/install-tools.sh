#!/bin/sh
set -e

SOLANA_VERSION="$1"
ANCHOR_VERSION="$2"
PLATFORM_TOOLS_VERSION="$3"

# Install Solana toolchain.
sh -c "$(curl -sSfL "https://release.anza.xyz/${SOLANA_VERSION}/install")"

# Install the platform-tools release the pinned anchor-cli builds with, which
# is not this cargo-build-sbf's own default. Installed here, before the cache
# save, so the entry carries it; otherwise every `anchor build` would
# download it afresh.
cargo-build-sbf --install-only --tools-version "$PLATFORM_TOOLS_VERSION"

# Run platform-tools SBF install script (cargo-build-sbf skips
# this at install time, so it cache misses at build time).
install.sh

# Install the stable crates.io anchor-cli at its exact pin.
CARGO_PROFILE_RELEASE_LTO=off cargo install anchor-cli \
	--version "$ANCHOR_VERSION" --locked --force
