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

import asyncio
import sys
from pathlib import Path

try:
    import iterm2
except ImportError:  # importable for unit-testing the pure ordering logic
    iterm2 = None

# Where the bash hooks record each session's state. Keep the prefix and the
# state hexes in sync with iterm-colors.sh.
STATE_PREFIX = "/tmp/iterm-color-"
_YELLOW_HEX = "3a2c08"  # permission request / file edit -> go approve
_GREEN_HEXES = {"080c2a", "082a0c"}  # reply wanted / attend mark

# Group ordering: yellow first, then green, then everything else.
_PRIORITY = {"yellow": 0, "green": 1, "neutral": 2}

POLL_SECONDS = 0.3


def _group_for_color(color: str) -> str:
    if color == _YELLOW_HEX:
        return "yellow"
    if color in _GREEN_HEXES:
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
