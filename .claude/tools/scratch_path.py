#!/usr/bin/env python3
"""Refuse a review artifact written directly into a shared temp root.

Every session on the machine shares ``/tmp`` (``/private/tmp`` on macOS),
``/var/tmp`` and the per-user ``tempfile.gettempdir()``. A review file named
``lens-completeness.md`` or ``review-diff.txt`` written straight into one of
them collides with a sibling session's file at the same path: one review's
``Write`` reported "updated" on a sibling's lens material and cost about ten
turns to unpick. So the tools that write review artifacts (``review_diff.py``,
``lens_preamble.py``) refuse an ``--out`` whose **parent** is a shared root.
A session-keyed subdirectory beneath one (``/tmp/eng-123/…``) is fine; that
is the private directory ``review-pr`` step 5 names.

Standard library only; tests live in ``tests/test_scratch_path.py``.
"""

from __future__ import annotations

import tempfile
from pathlib import Path

# The roots every session shares. `tempfile.gettempdir()` is added at call
# time, since it reads the environment.
SHARED_ROOTS = ("/tmp", "/private/tmp", "/var/tmp")


def _resolved(path: Path) -> Path:
    try:
        return path.expanduser().resolve()
    except OSError:
        return path.expanduser().absolute()


def shared_roots() -> set[Path]:
    """The shared temp roots, resolved (so ``/tmp`` and ``/private/tmp`` agree)."""
    return {_resolved(Path(p)) for p in (*SHARED_ROOTS, tempfile.gettempdir())}


def shared_root_refusal(out: str | Path, flag: str = "--out") -> str | None:
    """Why ``out`` must not be written, or ``None`` when it may be.

    Refused only when the file sits **directly** in a shared root; anything
    deeper is taken to be a session-private directory.
    """
    parent = _resolved(Path(out)).parent
    if parent not in shared_roots():
        return None
    return (
        f"{flag} {out} is directly under the shared temp root {parent}, where a "
        "sibling session's file of the same name is overwritten; write it to a "
        f"session-private directory instead (e.g. {parent}/eng-<n>/"
        f"{Path(out).name})"
    )
