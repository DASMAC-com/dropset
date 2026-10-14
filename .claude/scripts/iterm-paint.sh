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
# neither reads as "not waiting", which is the old green — never a silenced
# session that needed a reply.
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
      tool="$(json_field tool_name "$1")"
      case "$tool" in
        AskUserQuestion) printf '%s' "$STATE_REPLY" ;;
        Edit | Write | MultiEdit | NotebookEdit) printf '%s' "$STATE_PERMISSION" ;;
        *) printf '%s' "$STATE_NEUTRAL" ;;
      esac
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
      # A permission_prompt is yellow. The harness ALSO fires this for an
      # AskUserQuestion selector (from its side it is blocked on user input),
      # so this branch fires there too; the sticky-green guard in the main body
      # suppresses that companion yellow so the tool's green survives. Other
      # notification types leave the tab unchanged.
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

# Maintain the permission-wait sentinel. Refresh it on every permission_prompt
# yellow — the harness re-fires that notification while the prompt waits, so its
# mtime tracks "still waiting" — and clear it on any other paint. The monitor
# heals a STALE one back to neutral, which is what stops a guard-denied tool
# from leaving the tab yellow forever; see PERM_WAIT_STALE_SECONDS in
# iterm-colors.sh for why that safety net is needed and why it is generous.
# The clear is keyed on the COLOR, not on "anything that isn't a Notification".
# An edit tool's PreToolUse also paints yellow (see color_for_event), so an
# else-branch would delete a live sentinel the moment a Write followed an
# approved permission prompt — and the guard most likely to then deny that
# Write is the worktree edit-path guard, whose tool set is exactly those four.
# That reintroduced the wedge this whole mechanism exists to close, one step
# further along. Clearing only on a non-permission paint keeps the sentinel
# alive across a repaint while still dropping it the moment the tab goes
# neutral or green.
if [ "$EVENT" = "Notification" ] && [ "$COLOR" = "$STATE_PERMISSION" ]; then
  : >"$(perm_wait_path "$TTY_BASE")"
elif [ "$COLOR" != "$STATE_PERMISSION" ]; then
  rm -f "$(perm_wait_path "$TTY_BASE")"
fi

# A yellow is ARMED, never painted here: the monitor promotes it once it has
# outlived PERM_DEBOUNCE_SECONDS (see iterm-colors.sh for the flicker this
# stops). Two rules keep the debounce from restarting forever:
#   * a yellow already on screen is left alone — a re-fire is not a new prompt;
#   * an existing arm keeps its timestamp. The common order for an edit that
#     needs approval is the edit tool's PreToolUse (no message) THEN the
#     permission_prompt (with one), so the message is added to the arm while
#     its mtime is carried over.
if [ "$COLOR" = "$STATE_PERMISSION" ]; then
  [ "$(cat "$(state_file_path "$TTY_BASE")" 2>/dev/null)" = "$STATE_PERMISSION" ] && exit 0
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
rm -f "$(perm_pending_path "$TTY_BASE")"

echo "$COLOR" >"$STATE_PREFIX$TTY_BASE"
emit_set_colors "$COLOR" >"$TTY_PATH"
