#!/usr/bin/env python3
"""Read the CI-mirror manifest: each required check, and its local mirror.

``cfg/ci-mirror.json`` holds one row per required status context in the
ruleset::

    {"context": "Tests (Postgres)", "workflow": "test.yml",
     "ci": ["test-postgres"], "local": ["test-postgres"]}

``ci`` names the ``make`` targets the context's CI job runs; ``local`` names
the targets that mirror it on a workstation, run in order. Either may be
empty: ``Semantic PR`` and ``Detect changes`` run no target and have no
mirror. ``verify_head.py --full`` runs :func:`local_targets` as its stages,
so a CI job gains a local mirror by gaining a row here, never by an edit to
the tool.

Standard library only; not a Cargo workspace member (``CLAUDE.md`` → "Skill
tooling"). Tests: ``tests/test_ci_mirror.py``, via ``make tools-tests``.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

#: The committed manifest, from the repository root.
MANIFEST = Path(__file__).resolve().parents[2] / "cfg" / "ci-mirror.json"

_FIELDS = {"context": str, "workflow": str, "ci": list, "local": list}


class ManifestError(ValueError):
    """The manifest is missing, unparsable, or a row has the wrong shape."""


@dataclass(frozen=True)
class Row:
    context: str
    workflow: str
    ci: tuple[str, ...]
    local: tuple[str, ...]


def parse(text: str) -> list[Row]:
    """The manifest's rows, in file order; raises :class:`ManifestError`."""
    try:
        data = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ManifestError(f"not JSON: {exc}") from exc
    rows = data.get("contexts") if isinstance(data, dict) else None
    if not isinstance(rows, list) or not rows:
        raise ManifestError("expected a non-empty `contexts` list")
    out: list[Row] = []
    for i, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ManifestError(f"row {i} is not an object")
        for key, kind in _FIELDS.items():
            if not isinstance(row.get(key), kind):
                raise ManifestError(f"row {i}: `{key}` must be a {kind.__name__}")
        for key in ("ci", "local"):
            if not all(isinstance(t, str) and t for t in row[key]):
                raise ManifestError(f"row {i}: `{key}` must list target names")
        out.append(
            Row(row["context"], row["workflow"], tuple(row["ci"]), tuple(row["local"]))
        )
    seen = [r.context for r in out]
    dupes = sorted({c for c in seen if seen.count(c) > 1})
    if dupes:
        raise ManifestError(f"duplicate context(s): {', '.join(dupes)}")
    return out


def load(path: Path = MANIFEST) -> list[Row]:
    try:
        return parse(path.read_text())
    except OSError as exc:
        raise ManifestError(f"cannot read {path}: {exc}") from exc


def local_targets(rows: list[Row]) -> list[str]:
    """Every distinct local target, in first-mention order."""
    return list(dict.fromkeys(t for r in rows for t in r.local))


def contexts_for(rows: list[Row], target: str) -> frozenset[str]:
    """The contexts a local target mirrors: all must be green for CI to
    settle it."""
    return frozenset(r.context for r in rows if target in r.local)
