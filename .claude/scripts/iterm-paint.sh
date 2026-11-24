#!/bin/bash
# The hook painter. This is the single entry point every Claude Code hook calls.
# It derives the desired state color from the hook event on stdin, writes it to
# this session's state file, and applies it to the TTY immediately.
#
# Why one script (not one hook per color): matching PreToolUse hooks run in
# parallel with no ordering guarantee, so wiring `*`->neutral alongside
# `AskUserQuestion`->green raced and the tab color was non-deterministic. Here a
# single hook per event calls this painter, which picks the color itself, so the
# color is a deterministic function of the event.
#
# Usage:
#   iterm-paint.sh              # hook mode: read the hook JSON on stdin
#   iterm-paint.sh <state-hex>  # direct mode: paint this state now (attend etc.)

# shellcheck source=.claude/scripts/iterm-colors.sh
# shellcheck disable=SC1090,SC1091
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/iterm-colors.sh"

# Pull a JSON string field out of the hook payload without a jq dependency.
# Hook payloads are single-object JSON with flat string fields (tool_name,
# hook_event_name, notification_type), so a targeted regex is enough.
json_field() { # $1 = field name, $2 = payload
  local re="\"$1\"[[:space:]]*:[[:space:]]*\"([^\"]*)\""
  if [[ "$2" =~ $re ]]; then
    printf '%s' "${BASH_REMATCH[1]}"
  fi
}

# True when the payload carries a NON-EMPTY array under field $1. The closing
# quote must follow the name directly, so the same words quoted inside an
# escaped string (`last_assistant_message`) cannot match.
json_nonempty_array() { # $1 = field name, $2 = payload
  local re="\"$1\"[[:space:]]*:[[:space:]]*\\[[[:space:]]*[^][:space:]]"
  [[ "$2" =~ $re ]]
}

# A Stop whose session will resume by itself: a background task (a CI watch, a
# sub-agent fan-out, a merge-queue poll) or a scheduled wake-up is still in
# flight. Both lists are on the Stop payload. An older harness that sends
# neither reads as "not waiting", which is the old green.
#
# The known cost: anything long-lived in either list — a recurring `/loop`
# cron, a dev server left running in the background — makes EVERY Stop slate,
# including one that ends on a plain-text question. AskUserQuestion still
# paints its own green, which is the house route for a decision.
stop_is_self_waiting() { # $1 = payload
  json_nonempty_array background_tasks "$1" || json_nonempty_array session_crons "$1"
}

# Map a hook event to a state color. Prints nothing for events that should not
# repaint (e.g. a non-permission Notification), so the caller leaves the tab as
# it is.
color_for_event() { # $1 = payload
  local event tool notification_type
  event="$(json_field hook_event_name "$1")"
  case "$event" in
    PreToolUse)
      # No tool is yellow here. An edit tool once was, on the guess that it
      # might prompt, but most edits are auto-approved and many take longer
      # than the debounce to finish, so the guess surfaced yellow for edits
      # that never asked anything. A dialog that does reach the operator
      # fires PermissionRequest (below).
      tool="$(json_field tool_name "$1")"
      case "$tool" in
        AskUserQuestion) printf '%s' "$STATE_REPLY" ;;
        *) printf '%s' "$STATE_NEUTRAL" ;;
      esac
      ;;
    PermissionRequest)
      # The yellow trigger: the harness fires this the moment a permission
      # dialog is shown to the operator, and never for a call the auto-mode
      # classifier or an allow rule decides. An AskUserQuestion selector fires
      # it too; its green stands, so that case leaves the tab unchanged.
      tool="$(json_field tool_name "$1")"
      [ "$tool" = "AskUserQuestion" ] || printf '%s' "$STATE_PERMISSION"
      ;;
    PostToolUse | UserPromptSubmit) printf '%s' "$STATE_NEUTRAL" ;;
    Stop)
      # Green asks for the operator, so it is kept for a session that will not
      # move without one. A self-waiting session gets the quiet slate, which
      # the reorderer sorts with the neutral tabs rather than to the front.
      if stop_is_self_waiting "$1"; then
        printf '%s' "$STATE_WAITING"
      else
        printf '%s' "$STATE_REPLY"
      fi
      ;;
    Notification)
      notification_type="$(json_field notification_type "$1")"
      # A permission_prompt is yellow too. The harness sends it about six
      # seconds AFTER the PermissionRequest, for a dialog still unanswered,
      # and re-sends it while the dialog waits; it carries the message text
      # this integration delivers when it owns notifications. The harness ALSO
      # fires this for an AskUserQuestion selector (from its side it is blocked
      # on user input), so this branch fires there too; the sticky-green guard
      # in the main body suppresses that companion yellow so the tool's green
      # survives. Other notification types leave the tab unchanged.
      [ "$notification_type" = "permission_prompt" ] && printf '%s' "$STATE_PERMISSION"
      ;;
  esac
}

# --- AskUserQuestion "sticky green" ------------------------------------------
# The harness fires BOTH a PreToolUse(AskUserQuestion) — painted green (reply
# wanted) — AND a companion Notification(permission_prompt) for the same
# selector, and RE-fires that notification periodically while the selector
# waits for an answer. With no guard, each yellow overwrites the green by
# last-write and the tab misreads as "go approve". The permission_prompt
# payload carries no field that tells the AskUserQuestion companion apart from a
# genuine tool-permission prompt, so instead the AskUserQuestion green is made
# *sticky*: painting it drops a per-tty sentinel, and EVERY permission_prompt
# Notification is suppressed while that sentinel is present — until the selector
# is answered (its PostToolUse) or any other paint clears it. There is
# deliberately NO time window: a selector can wait indefinitely, so a fixed
# window would let a re-fired notification repaint yellow mid-wait (the bug this
# guard replaced). A stale sentinel from a crashed session is cleared by the
# next non-AskUserQuestion paint (below) and by iterm-start.sh at session start.
askq_sentinel_path() { printf '%s%s.askq' "$STATE_PREFIX" "$1"; } # $1 = tty base

# True while an unanswered AskUserQuestion green is in effect on this tty.
askq_sentinel_present() { [ -f "$(askq_sentinel_path "$1")" ]; } # $1 = tty base

COLOR="$1"
PAYLOAD=""
if [ -z "$COLOR" ]; then
  PAYLOAD="$(cat)"
  COLOR="$(color_for_event "$PAYLOAD")"
fi

EVENT="$(json_field hook_event_name "$PAYLOAD")"
NOTIFICATION_TYPE="$(json_field notification_type "$PAYLOAD")"
MESSAGE="$(json_field message "$PAYLOAD")"

# Notifications that do not repaint. When this integration owns notifications
# (see painter_owns_notifications), these are the ones it delivers — the
# needs-operator class. idle_prompt is the harness's "still waiting for you"
# nudge, and it is exactly the one a self-waiting session must not send, so it
# is dropped while the tab is slate. Every other type (auth_success,
# agent_completed, quota_*) asks nothing of the operator.
if [ -z "$COLOR" ]; then
  [ "$EVENT" = "Notification" ] || exit 0
  TTY_PATH=$(resolve_tty) || exit 0
  TTY_BASE="$(basename "$TTY_PATH")"
  case "$NOTIFICATION_TYPE" in
    idle_prompt)
      [ "$(cat "$(state_file_path "$TTY_BASE")" 2>/dev/null)" = "$STATE_WAITING" ] && exit 0
      notify_operator "$MESSAGE" >"$TTY_PATH"
      ;;
    elicitation_dialog | elicitation_url_dialog | agent_needs_input)
      notify_operator "$MESSAGE" >"$TTY_PATH"
      ;;
  esac
  exit 0
fi

# Record the state (for the monitor and the attend toggle) and paint it now.
TTY_PATH=$(resolve_tty) || exit 0
TTY_BASE="$(basename "$TTY_PATH")"
ensure_monitor "$TTY_PATH"

# Suppress the AskUserQuestion companion permission_prompt yellow while the
# green sentinel says the selector is still awaiting a reply. This fires for
# every re-fired notification until the sentinel is cleared below. The FIRST
# companion still notifies — a selector is the plainest needs-operator case
# there is — and marks the sentinel so a re-fire does not notify again.
if [ "$EVENT" = "Notification" ] && [ "$COLOR" = "$STATE_PERMISSION" ] &&
  [ "$NOTIFICATION_TYPE" = "permission_prompt" ] &&
  askq_sentinel_present "$TTY_BASE"; then
  if [ ! -s "$(askq_sentinel_path "$TTY_BASE")" ]; then
    echo notified >"$(askq_sentinel_path "$TTY_BASE")"
    notify_operator "$MESSAGE" >"$TTY_PATH"
  fi
  exit 0
fi

# Maintain the sentinel: set it when painting an AskUserQuestion green, clear it
# on any other paint so a later genuine permission prompt is not suppressed.
if [ "$EVENT" = "PreToolUse" ] && [ "$(json_field tool_name "$PAYLOAD")" = "AskUserQuestion" ]; then
  : >"$(askq_sentinel_path "$TTY_BASE")"
else
  rm -f "$(askq_sentinel_path "$TTY_BASE")"
fi

# Maintain the permission-wait sentinel. Set it on the PermissionRequest that
# arms a yellow and refresh it on every permission_prompt — the harness re-fires
# that notification while the dialog waits, so its mtime tracks "still waiting"
# — and clear it on any non-yellow paint. The monitor heals a STALE one back to
# neutral, which is what stops a dialog answered with no later paint (a denial,
# or a tool a guard blocks after approval) from leaving the tab yellow forever;
# see PERM_WAIT_STALE_SECONDS in iterm-colors.sh for why that safety net is
# needed and why it is generous. A direct yellow (the attend toggle's, no event)
# carries none, so it is never healed.
if [ "$COLOR" = "$STATE_PERMISSION" ]; then
  case "$EVENT" in
    Notification | PermissionRequest) : >"$(perm_wait_path "$TTY_BASE")" ;;
  esac
else
  rm -f "$(perm_wait_path "$TTY_BASE")"
fi

# A yellow is ARMED, never painted here: the monitor promotes it once it has
# outlived PERM_DEBOUNCE_SECONDS (see iterm-colors.sh for why a dialog answered
# within the window never moves the tab). Two rules keep the debounce from
# restarting forever:
#   * a yellow already on screen is left alone — a re-fire is not a new prompt;
#   * an existing arm keeps its timestamp. The order for a dialog is the
#     PermissionRequest (no message) THEN, six seconds on, the permission_prompt
#     (with one), so the message is added to the arm while its mtime is carried
#     over.
# The message usually arrives after the promotion, the debounce being shorter
# than the harness's six seconds, so a yellow already on screen delivers it
# then — once, marked so a re-fire does not notify again.
# Direct mode (`iterm-paint.sh <hex>`) goes through the same two branches, so a
# direct yellow arms too, and any direct paint — the attend toggle's — cancels
# an arm, which reads as the operator acknowledging it.
if [ "$COLOR" = "$STATE_PERMISSION" ]; then
  if [ "$(cat "$(state_file_path "$TTY_BASE")" 2>/dev/null)" = "$STATE_PERMISSION" ]; then
    NOTIFIED="$(perm_notified_path "$TTY_BASE")"
    if [ -n "$MESSAGE" ] && [ ! -f "$NOTIFIED" ]; then
      : >"$NOTIFIED"
      notify_operator "$MESSAGE" >"$TTY_PATH"
    fi
    exit 0
  fi
  PENDING="$(perm_pending_path "$TTY_BASE")"
  if [ ! -f "$PENDING" ]; then
    printf '%s' "$MESSAGE" >"$PENDING"
  elif [ ! -s "$PENDING" ] && [ -n "$MESSAGE" ]; then
    touch -r "$PENDING" "$PENDING.ref"
    printf '%s' "$MESSAGE" >"$PENDING"
    touch -r "$PENDING.ref" "$PENDING"
    rm -f "$PENDING.ref"
  fi
  exit 0
fi
rm -f "$(perm_pending_path "$TTY_BASE")" "$(perm_notified_path "$TTY_BASE")"

echo "$COLOR" >"$STATE_PREFIX$TTY_BASE"
emit_set_colors "$COLOR" >"$TTY_PATH"
