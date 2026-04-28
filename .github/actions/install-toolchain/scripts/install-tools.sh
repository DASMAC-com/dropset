#!/bin/sh
set -e

SOLANA_VERSION="$1"
ANCHOR_REVISION="$2"

# Install Solana toolchain.
sh -c "$(curl -sSfL "https://release.anza.xyz/${SOLANA_VERSION}/install")"

# Install the default platform-tools that ships with this cargo-build-sbf.
cargo-build-sbf --install-only

# Run platform-tools SBF install script (cargo-build-sbf skips
# this at install time, so it cache misses at build time).
install.sh

# Install anchor-cli v2.
CARGO_PROFILE_RELEASE_LTO=off cargo install \
	--git https://github.com/solana-foundation/anchor.git \
	--rev "$ANCHOR_REVISION" \
	anchor-cli --locked --force
