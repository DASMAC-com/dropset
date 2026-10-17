#!/usr/bin/env python3
"""Fail when a crate has Docker-gated tests that the Postgres CI job never runs.

The default ``cargo nextest run`` skips ``#[ignore]``d tests, so a test gated on
a Docker daemon runs **only** in the ``Tests (Postgres)`` job, and only for the
crates that job names with ``-p``. A new crate whose suites carry the gate
attribute but that nobody adds to the selection is a test that says it needs
Docker and that CI never runs at all: every job stays green, and the coverage is
silently absent.

So the selection is checked against a set **derived from the tree**, never a
hand-kept list, which would only catch a regression of today's line:

* every tracked ``.rs`` file carrying the gate attribute
  (``#[ignore = "requires a Docker daemon …"]``) is mapped to its crate, the
  nearest ``Cargo.toml``, and that crate's ``[package]`` name;
* each such package must appear as a ``-p`` flag in the Makefile's
  ``test-postgres`` recipe, the selection's one owner;
* the workflow must still run ``make test-postgres``, so the owner is the
  thing CI actually executes.

A derivation that finds **zero** gated crates fails rather than passes: that
means the attribute's spelling drifted from :data:`GATE_RE`, and a guard that
matches nothing would approve every future omission.

Extra ``-p`` crates are fine — the selection also exists for feature-gated
unit tests that a plain run leaves off.

Usage::

    python3 .claude/tools/postgres_ci_wiring.py
    python3 .claude/tools/postgres_ci_wiring.py --show

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_postgres_ci_wiring.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

from review_diff import manifest_dir

#: The gate attribute, as every Docker-backed test in the tree spells it.
GATE_RE = re.compile(r'#\[ignore\s*=\s*"requires a Docker daemon')

#: The Makefile target that owns the Postgres job's selection.
TARGET = "test-postgres"

MAKEFILE = "Makefile"
WORKFLOW = ".github/workflows/test.yml"

_PACKAGE_NAME_RE = re.compile(r"""^\s*name\s*=\s*["']([^"']+)["']""", re.MULTILINE)
_SELECT_RE = re.compile(r"(?:^|\s)-p\s+(\S+)")
#: The workflow step that runs the target — a `run:` value, not a mention in a
#: comment or a longer target name.
_RUNS_TARGET_RE = re.compile(
    rf"""^\s*(?:-\s+)?run:\s*['"]?make {re.escape(TARGET)}['"]?\s*$""", re.MULTILINE
)


def tracked_rust_sources(root: Path) -> list[str]:
    """Every tracked ``.rs`` path under ``root``, repo-relative."""
    out = subprocess.run(
        ["git", "ls-files", "-z", "--", "*.rs"],
        cwd=root,
        stdout=subprocess.PIPE,
        check=True,
        text=True,
    ).stdout
    return [p for p in out.split("\0") if p]


def package_name(cargo_toml: str) -> str | None:
    """The ``name`` in a manifest's ``[package]`` table, or ``None``.

    Read from that table only: a ``[[bin]]`` or ``[lib]`` table also carries a
    ``name``, and in this tree one often precedes ``[package]``.
    """
    match = re.search(
        r"^\[package\][ \t]*(?:#[^\n]*)?$(.*?)(?=^\[|\Z)", cargo_toml, re.M | re.S
    )
    if not match:
        return None
    name = _PACKAGE_NAME_RE.search(match.group(1))
    return name.group(1) if name else None


def gated_packages(
    root: Path, sources: list[str]
) -> tuple[dict[str, list[str]], list[str]]:
    """Each package with a Docker-gated test, mapped to the files carrying one.

    Also returns the gated files whose package could not be resolved (no crate
    manifest, or none with a readable ``[package]`` name). Those are reported
    rather than skipped: a gated test this guard cannot place is exactly a test
    it cannot vouch for.
    """
    packages: dict[str, list[str]] = {}
    unresolved: list[str] = []
    for src in sources:
        try:
            text = (root / src).read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        if not GATE_RE.search(text):
            continue
        crate = manifest_dir(src, root)
        try:
            manifest = (root / crate / "Cargo.toml").read_text(encoding="utf-8")
        except OSError:
            manifest = ""
        name = package_name(manifest)
        if name:
            packages.setdefault(name, []).append(src)
        else:
            unresolved.append(src)
    gated = {name: sorted(files) for name, files in sorted(packages.items())}
    return gated, sorted(unresolved)


def recipe(makefile: str, target: str) -> str | None:
    """``target``'s recipe, continuation lines joined, or ``None`` if absent."""
    lines = makefile.splitlines()
    for i, line in enumerate(lines):
        if re.match(rf"^{re.escape(target)}\s*:(?!=)", line):
            body = []
            for follow in lines[i + 1 :]:
                if not follow.startswith("\t"):
                    break
                body.append(follow.strip().rstrip("\\"))
            return " ".join(body)
    return None


def selected_packages(recipe_text: str) -> list[str]:
    """The ``-p`` packages a recipe selects."""
    return sorted(set(_SELECT_RE.findall(recipe_text)))


def check(root: Path, sources: list[str] | None = None) -> list[str]:
    """Every wiring problem, as a human-readable line; empty means wired."""
    if sources is None:
        sources = tracked_rust_sources(root)
    problems = []

    gated, unresolved = gated_packages(root, sources)
    if not gated and not unresolved:
        problems.append(
            "no crate carries a Docker-gated test — the attribute's spelling "
            "drifted from GATE_RE, so this guard would approve any omission"
        )
    for src in unresolved:
        problems.append(
            f"{src} has a Docker-gated test but no crate `[package]` name could "
            f"be read for it, so its selection cannot be checked"
        )

    try:
        makefile = (root / MAKEFILE).read_text(encoding="utf-8")
    except OSError as exc:
        return problems + [f"cannot read {MAKEFILE}: {exc}"]
    body = recipe(makefile, TARGET)
    if body is None:
        return problems + [f"{MAKEFILE} has no `{TARGET}` target"]

    selected = set(selected_packages(body))
    for name, files in gated.items():
        if name not in selected:
            problems.append(
                f"{name} has Docker-gated tests ({', '.join(files)}) but "
                f"`make {TARGET}` does not select it — add `-p {name}`"
            )

    try:
        workflow = (root / WORKFLOW).read_text(encoding="utf-8")
    except OSError as exc:
        return problems + [f"cannot read {WORKFLOW}: {exc}"]
    if not _RUNS_TARGET_RE.search(workflow):
        problems.append(
            f"{WORKFLOW} no longer runs `make {TARGET}`, so the selection this "
            f"guard checks is not the one CI executes"
        )
    return problems


def find_root() -> Path:
    out = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        stdout=subprocess.PIPE,
        check=True,
        text=True,
    ).stdout
    return Path(out.strip())


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="postgres_ci_wiring.py",
        description=(
            "Assert every crate with a Docker-gated test is selected by the "
            f"`{TARGET}` target the Postgres CI job runs."
        ),
    )
    parser.add_argument(
        "--root", default=None, help="checkout root (default: the git toplevel)"
    )
    parser.add_argument(
        "--show",
        action="store_true",
        help="print the derived crates and the target's selection, then exit 0",
    )
    args = parser.parse_args(argv)
    root = Path(args.root) if args.root else find_root()

    if args.show:
        gated, unresolved = gated_packages(root, tracked_rust_sources(root))
        for src in unresolved:
            print(f"(unresolved) {src}")
        for name, files in gated.items():
            print(f"{name}: {len(files)} gated file(s)")
        body = recipe((root / MAKEFILE).read_text(encoding="utf-8"), TARGET) or ""
        print(f"make {TARGET} selects: {', '.join(selected_packages(body))}")
        return 0

    problems = check(root)
    if problems:
        print("\n".join(f"postgres-ci-wiring: {p}" for p in problems), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
