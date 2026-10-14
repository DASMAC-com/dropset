#!/bin/bash
# Start the per-TTY monitor for this session if it is not already running, and
# set the initial neutral state. Called by the SessionStart hook.

# shellcheck source=.claude/scripts/iterm-colors.sh
# shellcheck disable=SC1090,SC1091
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/iterm-colors.sh"

TTY_PATH=$(resolve_tty) || exit 0
TTY_NAME=$(basename "$TTY_PATH")

# Don't start a second monitor for this TTY. Unlike the old early exit, a live
# monitor no longer skips the reseed below: a resumed session must not inherit
# a stale arm or sentinel just because its monitor survived.
ensure_monitor "$TTY_PATH"

# SessionStart also fires on `compact`, which is the SAME session carrying on:
# reseeding there would turn a green tab neutral and drop a live arm or
# selector sentinel mid-turn. The payload's `source` names the case.
PAYLOAD=""
[ -t 0 ] || PAYLOAD="$(cat)" # a hand run from a terminal has no payload
SOURCE_RE='"source"[[:space:]]*:[[:space:]]*"compact"'
[[ "$PAYLOAD" =~ $SOURCE_RE ]] && exit 0

# Seed the neutral state so the monitor paints something immediately, and drop
# the per-TTY sentinels a crashed prior session may have left here (see
# iterm-paint.sh). The sticky-green one would otherwise suppress this session's
# first real permission prompt; the permission-wait one would make a brand new
# session's first heal check read a dead prompt's timestamp; a leftover
# debounce arm would promote a dead prompt to yellow.
echo "$STATE_NEUTRAL" >"$STATE_PREFIX$TTY_NAME"
rm -f "$STATE_PREFIX$TTY_NAME.askq"
rm -f "$STATE_PREFIX$TTY_NAME.permwait"
rm -f "$(perm_pending_path "$TTY_NAME")"
