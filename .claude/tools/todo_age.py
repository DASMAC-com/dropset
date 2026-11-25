#!/usr/bin/env python3
"""todo_age.py — the Todo age bump's rule, pure: no network, no MCP.

The scheduler dispatches on priorities as they stand, and nothing used to read
how long an item had stood, so a Low item under a steady flow of High and
Urgent work was never reached. The curator's bootstrap now raises a stale
Todo item one level, through ``board_batch.py aged``; this module is the rule
that subcommand applies, kept apart from the board tool so it is testable
without a transport and so the board tool stays the writer.

The rule (``docs/conventions/linear-automation.md`` → "Priorities: shared
authority, the agreement rule"):

* **The clock** runs from the later of two events in the issue's history: its
  last move into Todo, and its last priority change. A bump is a priority
  change, so it restarts the clock, and so does anyone's counter-write. An
  issue created straight into Todo has no state entry in its history (Linear
  records none at creation), so a **complete** history with no Todo entry
  dates the entry from the issue's creation — which is then its Todo entry,
  not Backlog time.
* **The bump**: a clock at or past the threshold raises the priority one
  level — None to Low to Medium to High. It never writes Urgent.
* **Skipped**: a blocked item (its clock is not reset), an item carrying the
  ``[Operator]`` title token, an item already High or Urgent.
* **A lowered item is not exempt**: the decrease restarts the clock, and once
  the threshold passes again it is raised like any other.
* A history that failed to read or came back truncated is **age unknown** and
  never bumped: an under-read history could hide the event that restarted
  the clock.

The threshold is ``DS_TODO_AGE_BUMP_DAYS``. Its default lives in the runtime
config manifest's row and is read from there, so no tool, skill or doc states
the number; unset with no default resolved, or not a positive whole number,
bumps nothing and says why in one line.

Standard library only; tests in ``tests/test_todo_age.py``.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path

#: The knob's name, read from the environment.
THRESHOLD_VAR = "DS_TODO_AGE_BUMP_DAYS"

#: The manifest whose row carries the knob's default.
MANIFEST = (
    Path(__file__).resolve().parents[2] / ".claude/shell/runtime-config.manifest.json"
)

#: The workflow state whose entry starts the clock, matched by name as the
#: board listing's ``--state Todo`` is.
TODO_STATE = "Todo"

#: The title token that marks a hand-executed item, fixed at its priority.
OPERATOR_TOKEN = "[Operator]"

#: One level up Linear's scale (0 none, 1 urgent, 2 high, 3 medium, 4 low).
#: High and Urgent have no entry: nothing is raised to Urgent.
NEXT_LEVEL = {0: 4, 4: 3, 3: 2}

#: The selection the clock reads, spliced into the board tool's Todo query.
#: ``pageInfo`` is load-bearing: it is how a truncated history is told apart
#: from a complete one.
HISTORY_SELECTION = """
      createdAt
      history(first: $historyFirst) {
        pageInfo { hasNextPage }
        nodes {
          createdAt
          fromPriority
          toPriority
          fromStateId
          toStateId
          toState { name }
        }
      }
"""

#: How many history entries one issue reads. The live board's longest Todo
#: history measured 28 entries when this was written; an issue past this is
#: reported age unknown rather than read short.
HISTORY_FIRST = 100


def resolve_threshold(
    env, manifest_path: Path = MANIFEST
) -> tuple[int | None, str | None]:
    """The threshold in days, or ``None`` and the one-line reason."""
    raw = (env.get(THRESHOLD_VAR) or "").strip()
    source = THRESHOLD_VAR
    if not raw:
        raw = manifest_default(manifest_path) or ""
        source = f"the manifest default for {THRESHOLD_VAR}"
        if not raw:
            return None, (
                f"{THRESHOLD_VAR} is unset and the manifest resolves no default "
                "— bumping nothing"
            )
    if not raw.isdecimal() or int(raw) <= 0:
        return None, (
            f"{source} is {raw!r}, not a positive whole number — bumping nothing"
        )
    return int(raw), None


def manifest_default(path: Path) -> str | None:
    """The knob row's ``default``, or ``None`` when the manifest or the row
    cannot be read."""
    try:
        rows = json.loads(path.read_text(encoding="utf-8")).get("variables") or []
    except (OSError, ValueError, AttributeError):
        return None
    for row in rows:
        if isinstance(row, dict) and row.get("name") == THRESHOLD_VAR:
            value = row.get("default")
            return None if value is None else str(value)
    return None


def parse_time(raw) -> datetime | None:
    """A Linear ``DateTime`` as an aware datetime, or ``None``."""
    if not isinstance(raw, str) or not raw:
        return None
    try:
        return datetime.fromisoformat(raw.replace("Z", "+00:00"))
    except ValueError:
        return None


def clock_start(issue: dict) -> tuple[datetime | None, str]:
    """When the issue's clock started, and which event started it — or
    ``None`` and why the age is unknown."""
    history = issue.get("history")
    if not isinstance(history, dict) or not isinstance(history.get("nodes"), list):
        return None, "history unread"
    if (history.get("pageInfo") or {}).get("hasNextPage"):
        return None, "history truncated"
    entered: datetime | None = None
    changed: datetime | None = None
    for node in history["nodes"]:
        when = parse_time((node or {}).get("createdAt"))
        if when is None:
            return None, "history entry undated"
        to_state = (node.get("toState") or {}).get("name")
        if to_state == TODO_STATE and node.get("fromStateId") != node.get("toStateId"):
            entered = max(entered, when) if entered else when
        to_priority = node.get("toPriority")
        if to_priority is not None and to_priority != node.get("fromPriority"):
            changed = max(changed, when) if changed else when
    if entered is None:
        entered = parse_time(issue.get("createdAt"))
        if entered is None:
            return None, "no Todo entry and no creation date"
        entered_why = "created in Todo"
    else:
        entered_why = "entered Todo"
    if changed is not None and changed >= entered:
        return changed, "priority changed"
    return entered, entered_why


@dataclass(frozen=True)
class Aged:
    """One Todo issue's verdict: a bump, an unknown age, or a skip."""

    identifier: str
    priority: int
    age_days: int | None
    proposed: int | None
    note: str


def judge(
    issue: dict,
    *,
    days: int,
    now: datetime,
    blocked: bool,
    tokens: tuple[str, ...],
) -> Aged:
    """The verdict for one Todo issue. ``proposed`` is set only on a bump."""
    ident = str(issue.get("identifier") or "?")
    priority = int(issue.get("priority") or 0)

    def verdict(note: str, age=None, proposed=None) -> Aged:
        return Aged(ident, priority, age, proposed, note)

    if blocked:
        return verdict("skipped: blocked")
    if OPERATOR_TOKEN in tokens:
        return verdict("skipped: operator item")
    if priority not in NEXT_LEVEL:
        return verdict("skipped: already high or urgent")
    start, why = clock_start(issue)
    if start is None:
        return verdict(f"age unknown: {why}")
    age = now - start
    age_days = age.days
    if age < timedelta(days=days):
        return verdict(f"under threshold ({why})", age_days)
    return verdict(why, age_days, NEXT_LEVEL[priority])
