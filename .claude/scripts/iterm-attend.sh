#!/bin/bash
# Toggle this iTerm session's green "notification" tab, like mark-as-unread:
# first press clears it to neutral, the next sets it green again, and so on.
# Bound to a keyboard shortcut via iTerm2's Run Coprocess action. Requires
# .zshrc to register the $ITERM_SESSION_ID -> tty mapping (see the
# local-integrations convention doc).

# shellcheck source=.claude/scripts/iterm-colors.sh
# shellcheck disable=SC1090,SC1091
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/iterm-colors.sh"

TTY_PATH=""

# Primary: look up via session ID (registered by shell startup).
if [ -n "$ITERM_SESSION_ID" ]; then
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
CURRENT=$(cat "$STATE_FILE" 2>/dev/null)
if [ "$(bg_to_tab "$CURRENT")" != "default" ]; then
  NEXT="$STATE_NEUTRAL"
else
  NEXT="$STATE_MARK"
fi

echo "$NEXT" >"$STATE_FILE"
emit_set_colors "$NEXT" >"$TTY_PATH"
