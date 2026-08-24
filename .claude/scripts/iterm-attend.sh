#!/bin/bash
# Toggle this iTerm session's green "notification" tab, like mark-as-unread:
# first press clears it to neutral, the next sets it green again, and so on.
# Bound to a keyboard shortcut via iTerm2's Run Coprocess action. Requires
# .zshrc to register the $ITERM_SESSION_ID -> tty mapping (see the
# local-integrations convention doc).
#
# Usage: iterm-attend.sh [--tty <path>] [--mark]
#
#   --tty <path>  act on THAT tty instead of this process's own. For marking a
#                 tab the caller does not live in -- the fleet-resume launcher
#                 marks each tab it opens, and a coprocess bound to a key can
#                 only ever reach its own session.
#   --mark        SET the green mark rather than toggling. A toggle happens to
#                 set green on a fresh tab (no state file reads as neutral), but
#                 relying on that makes the outcome depend on history: a tab
#                 already marked would be cleared. A launcher wants "green",
#                 not "the other one".
#
# With neither flag the behavior is exactly as before: toggle, this session.

# shellcheck source=.claude/scripts/iterm-colors.sh
# shellcheck disable=SC1090,SC1091
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/iterm-colors.sh"

TTY_PATH=""
FORCE_MARK=0

while [ $# -gt 0 ]; do
  case "$1" in
    --tty)
      # The `[ $# -ge 2 ]` guard is load-bearing, not defensive padding: in
      # bash a `shift 2` with only one positional left FAILS and leaves the
      # parameters UNCHANGED, so `--tty` as the final argument would spin this
      # loop forever. The script is bound to a keyboard shortcut via Run
      # Coprocess, so a hang is silent. `bash -n` proves syntax, not this.
      [ $# -ge 2 ] || {
        echo "iterm-attend.sh: --tty needs a path" >&2
        exit 2
      }
      TTY_PATH="$2"
      shift 2
      ;;
    --mark)
      FORCE_MARK=1
      shift
      ;;
    *)
      echo "iterm-attend.sh: unknown option: $1" >&2
      exit 2
      ;;
  esac
done

# Primary: look up via session ID (registered by shell startup).
if [ -z "$TTY_PATH" ] && [ -n "$ITERM_SESSION_ID" ]; then
  # Key by the stable session UUID only. The full $ITERM_SESSION_ID carries a
  # wNtNpN window/tab/pane prefix that changes when a pane is moved/split, so a
  # fresh coprocess sees a different prefix than the shell registered under.
  TTY_PATH=$(cat "$SESSION_TTY_PREFIX${ITERM_SESSION_ID##*:}" 2>/dev/null)
fi

# Fallback: tty command (works when run directly, not as a coprocess).
if [ -z "$TTY_PATH" ] || [ ! -c "$TTY_PATH" ]; then
  T=$(tty 2>/dev/null)
  [ "$T" != "not a tty" ] && TTY_PATH="$T"
fi

[ -z "$TTY_PATH" ] && exit 1
[ ! -c "$TTY_PATH" ] && exit 1

t=$(basename "$TTY_PATH")
STATE_FILE="$STATE_PREFIX$t"

# Mark-as-unread toggle: if the tab shows any attention color (green or yellow),
# clear it to neutral; otherwise set the green mark. Repeats on each press.
# `--mark` skips the toggle and sets green outright.
if [ "$FORCE_MARK" = "1" ]; then
  NEXT="$STATE_MARK"
else
  CURRENT=$(cat "$STATE_FILE" 2>/dev/null)
  if [ "$(bg_to_tab "$CURRENT")" != "default" ]; then
    NEXT="$STATE_NEUTRAL"
  else
    NEXT="$STATE_MARK"
  fi
fi

echo "$NEXT" >"$STATE_FILE"
emit_set_colors "$NEXT" >"$TTY_PATH"
