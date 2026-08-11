#!/bin/sh
set -e

# Decide whether to install from what is actually on disk, rather than from
# the cache lookup. An entry can be saved under this key while missing a
# binary (see the action's restore step for how that happened), and a hit on
# such an entry used to skip the install and fail the toolchain check on
# every run from then on, permanently: the key only changes when
# SOLANA_VERSION / ANCHOR_REV change, so nothing ever reinstalls. Probing
# with `make check-toolchain` keeps one definition of a usable toolchain, and
# catches a binary that is present but broken or on the wrong version, not
# only an absent one.
echo 'Probing the restored toolchain; a failure here only means reinstall.'
if make check-toolchain; then
	complete='true'
else
	complete='false'
fi
echo "complete=${complete}" >>"$GITHUB_OUTPUT"

# Cache keys are immutable, so a hit that probes incomplete can't be repaired
# by re-saving: every run reinstalls until the entry is purged. Save only on a
# genuine miss, and flag the poisoned entry loudly instead.
save='false'
if [ "$complete" = 'false' ]; then
	if [ "$CACHE_HIT" = 'true' ]; then
		msg="cache entry ${TOOLCHAIN_CACHE_KEY} is incomplete, so every"
		msg="${msg} run reinstalls the toolchain. Keys are immutable —"
		msg="${msg} purge it: gh cache delete ${TOOLCHAIN_CACHE_KEY}"
		echo "::warning::${msg}"
	else
		save='true'
	fi
fi
echo "save=${save}" >>"$GITHUB_OUTPUT"
