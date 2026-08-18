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

# Delete every entry stored under this key, then report whether the key came
# out clear. Confirming with a follow-up read rather than trusting the
# delete's exit status is what makes this safe to run from all three
# toolchain jobs of one workflow at once: the first delete wins and the
# losers get a 404, which by exit code alone is indistinguishable from a
# permission failure but means the opposite. `ref` is deliberately omitted so
# the delete spans every scope the key was ever saved under rather than only
# this run's.
purge_entry() {
	gh api --method DELETE \
		"repos/${GITHUB_REPOSITORY}/actions/caches?key=${TOOLCHAIN_CACHE_KEY}" \
		>/dev/null 2>&1 || true
	remaining=$(gh api \
		"repos/${GITHUB_REPOSITORY}/actions/caches?key=${TOOLCHAIN_CACHE_KEY}" \
		--jq '.actions_caches | length' 2>/dev/null) || return 1
	[ "$remaining" = '0' ]
}

# Cache keys are immutable, so a hit that probes incomplete can't be repaired
# by re-saving over it: the entry has to be deleted and then rewritten. Only a
# default-branch push does both halves usefully, because a save is scoped to
# the ref that made it — a PR run could delete the shared main-scoped entry
# but would republish only into its own refs/pull/N/merge scope, leaving every
# other PR to reinstall until main caught up. Confining the purge to main also
# keeps a PR that breaks `make check-toolchain` from deleting the entry every
# other run is relying on. A purge that fails for any reason (no `actions:
# write`, no `gh`, a lost race that somehow left the key populated) falls back
# to exactly the old behavior: warn, reinstall, don't save. It must never fail
# the job.
#
# One deliberate gap, recorded so it isn't re-litigated: a runner that already
# has a good toolchain but no cache entry probes complete and so never seeds
# the key. That is unreachable on ubuntu-latest, where nothing pre-installs
# solana or anchor, and the obvious generalization — save whenever the key
# missed — would attempt a save on a self-hosted runner whose tools sit
# outside the three cached paths, where a zero-matching-path save raises a
# hard ValidationError rather than a warning. Revisit only if self-hosted
# runners are ever adopted.
save='false'
if [ "$complete" = 'false' ]; then
	if [ "$CACHE_HIT" != 'true' ]; then
		save='true'
	elif [ "$PURGE_ALLOWED" = 'true' ] && purge_entry; then
		echo "Purged incomplete cache entry ${TOOLCHAIN_CACHE_KEY}; this run"
		echo 'republishes it from the install below.'
		save='true'
	else
		msg="cache entry ${TOOLCHAIN_CACHE_KEY} is incomplete, so every"
		msg="${msg} run reinstalls the toolchain. Keys are immutable —"
		msg="${msg} purge it: gh cache delete ${TOOLCHAIN_CACHE_KEY}"
		echo "::warning::${msg}"
	fi
fi
echo "save=${save}" >>"$GITHUB_OUTPUT"
