#!/usr/bin/env python3
"""Say whether a conformance-vector regeneration changed behavior or only bytes.

Every on-chain layout change regenerates ``sdk/conformance/``, and the swap
vectors embed whole market accounts as one-number-per-line byte arrays. The
review question is one bit — *did any expected output move, or only the embedded
account bytes?* — and the only conforming way to answer it was a ``git diff`` of
the file, measured at ~3.4k tokens of byte-array hunks. An inline JSON compare
is an unfirmable ``python3 -c``. This is that compare, committed::

    # Every vector file, HEAD against the working tree (the post-regenerate case).
    python3 .claude/tools/vector_diff.py

    # One file, against the merge base.
    python3 .claude/tools/vector_diff.py --ref origin/main \\
        sdk/conformance/simulate_swap_vectors.json

    # Every changed field, not the first few.
    python3 .claude/tools/vector_diff.py --all

Prints one line per file — ``unchanged``, ``bytes-only`` (naming the byte arrays
that moved) or ``fields`` (naming the changed paths, capped unless ``--all``).
A **byte array** is a list of at least ``MIN_BYTE_ARRAY`` integers, each in
``0..=255``: account data, never an expected output. ``_comment`` is prose and
reported as such, never as a field.

Exit 0 when no field changed (``unchanged`` / ``bytes-only``), 1 when one did,
2 on error. Stdlib only, and deliberately **not** a Cargo workspace member — see
``docs/conventions/skill-tooling.md``. Tests live in
``tests/test_vector_diff.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import sys

import show_at_ref

VECTOR_GLOB = "sdk/conformance/*.json"

# Shortest integer list read as account bytes. Real accounts run to hundreds of
# bytes; an expected-output list (a handful of sizes or prices) sits far below.
MIN_BYTE_ARRAY = 32

# How many changed field paths a `fields` line names without `--all`.
DEFAULT_LIMIT = 8


class VectorDiffError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def is_byte_array(value) -> bool:
    return (
        isinstance(value, list)
        and len(value) >= MIN_BYTE_ARRAY
        and all(
            isinstance(x, int) and not isinstance(x, bool) and 0 <= x <= 255
            for x in value
        )
    )


def _named(items: list) -> dict | None:
    """Key a list of cases by ``name`` when every entry carries a unique one.

    Matching by name keeps an inserted case from shifting every later index into
    a spurious change.
    """
    if not items or not all(isinstance(x, dict) and "name" in x for x in items):
        return None
    keyed = {x["name"]: x for x in items}
    return keyed if len(keyed) == len(items) else None


def compare(old, new, path: str, fields: list[str], byte_arrays: list[str]) -> None:
    """Append every difference under ``path`` to ``fields`` or ``byte_arrays``."""
    if is_byte_array(old) or is_byte_array(new):
        if old != new:
            byte_arrays.append(path or "<root>")
        return
    if isinstance(old, dict) and isinstance(new, dict):
        for key in sorted(old.keys() | new.keys()):
            sub = f"{path}.{key}" if path else key
            if key not in new:
                fields.append(f"{sub} (removed)")
            elif key not in old:
                fields.append(f"{sub} (added)")
            else:
                compare(old[key], new[key], sub, fields, byte_arrays)
        return
    if isinstance(old, list) and isinstance(new, list):
        old_named, new_named = _named(old), _named(new)
        if old_named is not None and new_named is not None:
            compare(old_named, new_named, path, fields, byte_arrays)
            return
        for i in range(max(len(old), len(new))):
            sub = f"{path}[{i}]"
            if i >= len(new):
                fields.append(f"{sub} (removed)")
            elif i >= len(old):
                fields.append(f"{sub} (added)")
            else:
                compare(old[i], new[i], sub, fields, byte_arrays)
        return
    if old != new:
        fields.append(f"{path or '<root>'}: {_short(old)} -> {_short(new)}")


def _short(value) -> str:
    text = json.dumps(value)
    return text if len(text) <= 40 else text[:37] + "..."


def summarize(old: dict, new: dict, limit: int | None) -> tuple[str, bool]:
    """``(line, behavior_changed)`` for one file's pair of parsed documents."""
    fields: list[str] = []
    byte_arrays: list[str] = []
    prose = old.get("_comment") != new.get("_comment")
    compare(
        {k: v for k, v in old.items() if k != "_comment"},
        {k: v for k, v in new.items() if k != "_comment"},
        "",
        fields,
        byte_arrays,
    )
    notes = []
    if byte_arrays:
        notes.append(f"{len(byte_arrays)} byte array(s) moved")
    if prose:
        notes.append("_comment changed")
    if fields:
        shown = fields if limit is None else fields[:limit]
        more = len(fields) - len(shown)
        line = f"fields: {len(fields)} changed ({'; '.join(shown)}"
        line += f"; +{more} more, --all to list)" if more else ")"
        if notes:
            line += f" [{', '.join(notes)}]"
        return line, True
    if byte_arrays:
        line = f"bytes-only: {len(byte_arrays)} byte array(s) moved ({', '.join(byte_arrays)})"
        if prose:
            line += " [_comment changed]"
        return line, False
    return ("unchanged [_comment changed]" if prose else "unchanged"), False


def _parse(text: str, label: str) -> dict:
    try:
        doc = json.loads(text)
    except json.JSONDecodeError as exc:
        raise VectorDiffError(f"{label} is not valid JSON: {exc}") from exc
    if not isinstance(doc, dict):
        raise VectorDiffError(f"{label} is not a JSON object")
    return doc


def diff_file(ref: str, path: str, limit: int | None) -> tuple[str, bool]:
    try:
        old_text = show_at_ref.read_blob(ref, path)
    except show_at_ref.ShowAtRefError as exc:
        raise VectorDiffError(str(exc)) from exc
    try:
        with open(path, encoding="utf-8") as handle:
            new_text = handle.read()
    except OSError as exc:
        raise VectorDiffError(f"cannot read {path}: {exc}") from exc
    return summarize(_parse(old_text, f"{ref}:{path}"), _parse(new_text, path), limit)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="vector_diff.py",
        description="Did a conformance-vector regeneration change behavior or only bytes?",
    )
    parser.add_argument(
        "paths",
        nargs="*",
        help=f"vector files (default: every {VECTOR_GLOB})",
    )
    parser.add_argument(
        "--ref", default="HEAD", help="the git ref to compare against (default HEAD)"
    )
    parser.add_argument(
        "--all", action="store_true", help="name every changed field, not the first few"
    )
    return parser


def run(argv: list[str]) -> int:
    args = build_parser().parse_args(argv[1:])
    paths = args.paths or sorted(glob.glob(VECTOR_GLOB))
    if not paths:
        raise VectorDiffError(f"no files match {VECTOR_GLOB}; run from the repo root")
    limit = None if args.all else DEFAULT_LIMIT
    changed = False
    for path in paths:
        line, behavior = diff_file(args.ref, path, limit)
        changed |= behavior
        print(f"{os.path.basename(path)}: {line}")
    return 1 if changed else 0


def main() -> int:
    try:
        return run(sys.argv)
    except VectorDiffError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
