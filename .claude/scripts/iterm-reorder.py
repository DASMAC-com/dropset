#!/usr/bin/env python3
"""FIFO attention-ordering of iTerm2 tabs (prototype).

A single app-level daemon that keeps each iTerm2 window's tabs sorted into
attention groups so you can park at position 1 and sweep right:

    [ yellow (permission) … ] [ green (reply wanted) … ] [ everything else … ]

Within each attention group the order is **FIFO** — the oldest tab to enter
the group stays leftmost, and a tab that newly needs attention goes to the
*back* of its group (just before the next group). So position 1 is always the
thing that has waited longest; clear it, it drops out, and the next-oldest
slides into position 1.

This reads the per-TTY color state the bash hooks already write to
`/tmp/iterm-color-<tty>` (see `iterm-colors.sh`) and maps each tab to it via the
session's `tty` variable. Reordering itself is only possible through iTerm2's
**Python API** (`window.async_set_tabs`) — no escape sequence moves a tab — so
this half of the integration is Python, not bash, and is a separate process
from the per-tty color monitor.

Runtime: needs iTerm2's Python API enabled (Prefs → General → Magic → Enable
Python API) and the `iterm2` package. Run it as a long-lived script — drop it in
`~/Library/Application Support/iTerm2/Scripts/AutoLaunch/` (iTerm2 manages its
venv) or run it by hand in a venv with `iterm2` installed. See the
local-integrations convention doc.
"""

# cspell:word ttys

from __future__ import annotations

import asyncio
import re
import sys
from pathlib import Path

try:
    import iterm2
except ImportError:  # importable for unit-testing the pure ordering logic
    iterm2 = None

# Where the bash hooks record each session's state.
STATE_PREFIX = "/tmp/iterm-color-"

# The palette is READ from iterm-colors.sh rather than copied here. It used to
# be duplicated, and the duplication was a silent trap: `_group_for_color`
# falls through to "neutral" for any hex it does not recognize, so recoloring
# the palette — which the convention doc explicitly invites, saying "edit this
# file to recolor everything" — left the colors correct and killed FIFO
# attention ordering outright, with nothing to see. These values are only a
# last-resort fallback for a deploy where the palette cannot be located.
_FALLBACK_PALETTE = {
    "yellow": frozenset({"3a2c08"}),  # permission request / file edit
    "green": frozenset({"080c2a", "082a0c"}),  # reply wanted / attend mark
}

# `STATE_PERMISSION` is the yellow; `STATE_REPLY` and `STATE_MARK` are both
# greens. `STATE_NEUTRAL` needs no entry — it is the fall-through.
_PALETTE_KEYS = {
    "yellow": ("STATE_PERMISSION",),
    "green": ("STATE_REPLY", "STATE_MARK"),
}

_STATE_ASSIGNMENT = re.compile(r'^(STATE_[A-Z]+)="([0-9a-fA-F]{6})"', re.MULTILINE)

# Group ordering: yellow first, then green, then everything else.
_PRIORITY = {"yellow": 0, "green": 1, "neutral": 2}

POLL_SECONDS = 0.3


def palette_source() -> Path | None:
    """Where to read `iterm-colors.sh` from, or None if it cannot be found.

    A sibling first, which covers both the in-checkout layout and the
    `~/.claude/scripts/` global deploy (the deploy step copies the whole
    family). The explicit home path covers the iTerm2 `AutoLaunch/` deploy,
    where only this file is copied and there are no siblings.
    """
    for candidate in (
        Path(__file__).resolve().parent / "iterm-colors.sh",
        Path.home() / ".claude" / "scripts" / "iterm-colors.sh",
    ):
        if candidate.is_file():
            return candidate
    return None


def load_palette(path: Path | None = None) -> tuple[dict, bool]:
    """Return ``(palette, from_file)`` — the state hexes and whether they came
    from `iterm-colors.sh` rather than the fallback.

    Reports the provenance instead of swallowing it: a reorderer silently
    running on stale fallback hexes is the very failure this replaced, so the
    caller announces which source it got.
    """
    path = palette_source() if path is None else path
    if path is None:
        return {k: set(v) for k, v in _FALLBACK_PALETTE.items()}, False
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return {k: set(v) for k, v in _FALLBACK_PALETTE.items()}, False

    found = {name: value.lower() for name, value in _STATE_ASSIGNMENT.findall(text)}
    palette = {}
    for group, names in _PALETTE_KEYS.items():
        palette[group] = {found[n] for n in names if n in found}
    # A partial read is worse than no read: it would silently demote whichever
    # state went missing to "neutral". Fall back whole rather than in part.
    if not all(palette.values()):
        return {k: set(v) for k, v in _FALLBACK_PALETTE.items()}, False
    return palette, True


# Loaded once at import. A palette edit therefore needs the reorderer
# restarted — unlike `iterm-monitor.sh`, which re-reads on every state change.
PALETTE, PALETTE_FROM_FILE = load_palette()


def _group_for_color(color: str, palette: dict | None = None) -> str:
    palette = PALETTE if palette is None else palette
    color = color.strip().lower()
    if color in palette["yellow"]:
        return "yellow"
    if color in palette["green"]:
        return "green"
    return "neutral"


def _read_group(tty: str) -> str:
    """The attention group of the tab on ``tty`` (``/dev/ttysNNN``), from its
    state file. A missing file (a non-Claude tab) reads as neutral.
    """
    name = tty.rsplit("/", 1)[-1]
    try:
        color = Path(f"{STATE_PREFIX}{name}").read_text(encoding="utf-8").strip()
    except (OSError, ValueError):
        # Missing file, or a torn concurrent write (non-UTF-8) — both self-correct
        # on the next poll, so read as neutral rather than crashing.
        return "neutral"
    return _group_for_color(color)


def plan_order(entries, seq: dict, last_group: dict, counter: int):
    """Pure FIFO ordering. ``entries`` is the window's tabs in current order as
    ``(tab_id, group)`` pairs. Returns ``(order, counter)`` where ``order`` is
    the list of indices into ``entries`` in the desired left-to-right order and
    ``counter`` is the advanced global FIFO counter. ``seq`` / ``last_group`` are
    the persistent per-tab state, mutated in place.
    """
    ranked = []  # (priority, fifo_seq, original_index)
    for i, (tid, group) in enumerate(entries):
        # Assign a FIFO sequence when a tab *enters* an attention group; drop it
        # when it goes neutral. Staying in a group keeps its sequence, so it
        # holds its place while newer entries queue behind it.
        if group != last_group.get(tid):
            last_group[tid] = group
            if group in ("yellow", "green"):
                counter += 1
                seq[tid] = counter
            else:
                seq.pop(tid, None)
        ranked.append((_PRIORITY[group], seq.get(tid, 0), i))
    order = [i for _, _, i in sorted(ranked)]
    return order, counter


async def _reorder_window(window, seq: dict, last_group: dict, counter: int) -> int:
    """Reorder one window's tabs into FIFO attention groups. Returns the updated
    global FIFO counter.
    """
    tabs = list(window.tabs)
    entries = []  # (tab_id, group)
    for tab in tabs:
        session = tab.current_session
        group = "neutral"
        if session is not None:
            tty = await session.async_get_variable("tty")
            if tty:
                group = _read_group(tty)
        entries.append((tab.tab_id, group))

    order, counter = plan_order(entries, seq, last_group, counter)
    desired = [tabs[i] for i in order]
    if desired != tabs:
        # async_set_tabs preserves the selected tab, so a tab you're working in
        # moves to its queue slot but stays focused — it never steals focus.
        await window.async_set_tabs(desired)
    return counter, [tid for tid, _ in entries]


def _prune(seq: dict, last_group: dict, live: set) -> None:
    """Drop per-tab state for tabs that no longer exist, so the dicts don't grow
    unbounded over a long-running daemon.
    """
    for tid in list(seq):
        if tid not in live:
            del seq[tid]
    for tid in list(last_group):
        if tid not in live:
            del last_group[tid]


async def main(connection):
    # Say which palette is in force. Running on the fallback hexes still works
    # but silently drifts the moment someone recolors iterm-colors.sh, and a
    # silent drift here degrades to "no attention ordering at all".
    if PALETTE_FROM_FILE:
        print(f"iterm-reorder: palette read from {palette_source()}")
    else:
        print(
            "iterm-reorder: iterm-colors.sh not found or unreadable — using "
            "built-in fallback hexes. If the palette has been recolored, "
            "attention ordering will not recognize the new states.",
            file=sys.stderr,
        )

    app = await iterm2.async_get_app(connection)
    seq: dict = {}  # tab_id -> FIFO sequence within its current attention group
    last_group: dict = {}  # tab_id -> last-seen group (to detect entry)
    counter = 0
    while True:
        live: set = set()
        clean = True
        for window in app.terminal_windows:
            # A tab or window closing between the snapshot and async_set_tabs is a
            # real race; isolate it so one bad window never kills the daemon.
            try:
                counter, tab_ids = await _reorder_window(
                    window, seq, last_group, counter
                )
                live.update(tab_ids)
            except Exception as exc:  # noqa: BLE001 - daemon must survive any window error
                clean = False
                print(f"iterm-reorder: skipped a window: {exc}", file=sys.stderr)
        # Only prune when every window enumerated cleanly, so a transiently-erroring
        # window's live tabs aren't dropped (which would reset their FIFO slot).
        if clean:
            _prune(seq, last_group, live)
        await asyncio.sleep(POLL_SECONDS)


if __name__ == "__main__":
    iterm2.run_forever(main)
