#!/usr/bin/env python3
"""Fail when a crate has Docker-gated tests that the Postgres CI job never runs.

The default ``cargo nextest run`` skips ``#[ignore]``d tests, so a test gated on
a Docker daemon runs **only** in the ``Tests (Postgres)`` job, and only for the
crates the Makefile's ``POSTGRES_TEST_CRATES`` names. A crate whose suites carry
the gate attribute but that nobody adds to the list is a test that says it
needs Docker and that CI never runs at all: every job stays green, and the
coverage is silently absent.

So the selection is checked against a set **derived from the tree**, never a
hand-kept list, which would only catch a regression of today's line:

* every tracked ``.rs`` file carrying the gate attribute
  (``#[ignore = "requires a Docker daemon …"]``) is mapped to its crate, the
  nearest ``Cargo.toml``, and that crate's ``[package]`` name;
* each such package must be a word of ``POSTGRES_TEST_CRATES``, the
  selection's one owner, and the ``test-postgres`` recipe must expand that
  variable;
* the workflow must still run ``make test-postgres``, so the owner is the
  thing CI actually executes.

An empty gated set, with an empty list, is a legitimate state, not a failure.
What guards the attribute's spelling instead is the container library itself:
a tracked ``.rs`` file that holds a test and names ``testcontainers`` but
carries no gate attribute is reported, since it is a container-backed test
this guard cannot see. A shared harness module with no test of its own (a
``tests/common/mod.rs``) is not. The bound, stated plainly: a test that
reaches the container only through such a harness never names the library
itself, so a misspelled gate on it is invisible here, as is a test reaching
Postgres any other way, or one under an attribute other than ``#[test]`` /
``#[tokio::test]``. The drift check narrows the gap; it does not close it.

Extra crates in the list are fine; the guard checks only that no gated crate
is missing.

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

#: The container library a Docker-backed test reaches Postgres through.
CONTAINER_RE = re.compile(r"\btestcontainers")

#: A Rust test attribute: what makes a container user a test, not a harness.
TEST_ATTR_RE = re.compile(r"#\[(?:tokio::)?test\b")

#: The Makefile target the Postgres job runs.
TARGET = "test-postgres"

#: The Makefile variable that owns the job's crate selection.
VARIABLE = "POSTGRES_TEST_CRATES"

MAKEFILE = "Makefile"
WORKFLOW = ".github/workflows/test.yml"

_PACKAGE_NAME_RE = re.compile(r"""^\s*name\s*=\s*["']([^"']+)["']""", re.MULTILINE)
#: A make conditional directive, which may sit between a rule's recipe lines.
_CONDITIONAL_RE = re.compile(r"^(?:ifn?eq|ifn?def|else|endif)\b")
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


def classify_sources(
    root: Path, sources: list[str]
) -> tuple[dict[str, list[str]], list[str], list[str]]:
    """The gated packages, the unresolved gated files, and the ungated tests.

    Returns each package with a Docker-gated test mapped to the files carrying
    one; the gated files whose package could not be resolved (no crate
    manifest, or none with a readable ``[package]`` name), reported rather than
    skipped because a gated test this guard cannot place is one it cannot vouch
    for; and the files that hold a test and name the container library but
    carry no gate attribute. A harness module with no test is none of these.
    """
    packages: dict[str, list[str]] = {}
    unresolved: list[str] = []
    ungated: list[str] = []
    for src in sources:
        try:
            text = (root / src).read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        if not GATE_RE.search(text):
            if CONTAINER_RE.search(text) and TEST_ATTR_RE.search(text):
                ungated.append(src)
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
    return gated, sorted(unresolved), sorted(ungated)


def _logical_lines(makefile: str) -> list[str]:
    """The Makefile's lines with backslash continuations joined."""
    return re.sub(r"\\\n[ \t]*", " ", makefile).splitlines()


def recipe(makefile: str, target: str) -> str | None:
    """``target``'s recipe, continuations joined, or ``None`` if absent.

    Conditional directives (``ifeq`` … ``endif``) between recipe lines are
    stepped over, so every branch's lines are part of the result.
    """
    lines = _logical_lines(makefile)
    for i, line in enumerate(lines):
        if re.match(rf"^{re.escape(target)}\s*:(?!=)", line):
            body = []
            for follow in lines[i + 1 :]:
                if _CONDITIONAL_RE.match(follow):
                    continue
                if not follow.startswith("\t"):
                    break
                body.append(follow.strip())
            return " ".join(body)
    return None


def variable(makefile: str, name: str) -> list[str] | None:
    """The words of a simple ``name := …`` (or ``=``) assignment, or ``None``.

    A trailing ``#`` comment is not part of the value, so its words are never
    read as list entries.
    """
    for line in _logical_lines(makefile):
        match = re.match(rf"^{re.escape(name)}\s*:?=(.*)$", line)
        if match:
            return match.group(1).split("#", 1)[0].split()
    return None


def check(root: Path, sources: list[str] | None = None) -> list[str]:
    """Every wiring problem, as a human-readable line; empty means wired."""
    if sources is None:
        sources = tracked_rust_sources(root)
    problems = []

    gated, unresolved, ungated = classify_sources(root, sources)
    for src in unresolved:
        problems.append(
            f"{src} has a Docker-gated test but no crate `[package]` name could "
            f"be read for it, so its selection cannot be checked"
        )
    for src in ungated:
        problems.append(
            f"{src} uses testcontainers but carries no Docker gate attribute, "
            f"so this guard cannot place it — gate it with "
            f'`#[ignore = "requires a Docker daemon (Postgres container)"]`'
        )

    try:
        makefile = (root / MAKEFILE).read_text(encoding="utf-8")
    except OSError as exc:
        return problems + [f"cannot read {MAKEFILE}: {exc}"]
    body = recipe(makefile, TARGET)
    if body is None:
        return problems + [f"{MAKEFILE} has no `{TARGET}` target"]
    selected = variable(makefile, VARIABLE)
    if selected is None:
        return problems + [f"{MAKEFILE} has no `{VARIABLE}` assignment"]
    if f"$({VARIABLE})" not in body:
        problems.append(
            f"the `{TARGET}` recipe never expands `$({VARIABLE})`, so the list "
            f"this guard checks is not the one the target runs"
        )

    for name, files in gated.items():
        if name not in selected:
            problems.append(
                f"{name} has Docker-gated tests ({', '.join(files)}) but "
                f"`{VARIABLE}` does not name it — add `{name}`"
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
            "Assert every crate with a Docker-gated test is named by "
            f"`{VARIABLE}`, the list the Postgres CI job runs."
        ),
    )
    parser.add_argument(
        "--root", default=None, help="checkout root (default: the git toplevel)"
    )
    parser.add_argument(
        "--show",
        action="store_true",
        help="print the derived crates and the selection, then exit 0",
    )
    args = parser.parse_args(argv)
    root = Path(args.root) if args.root else find_root()

    if args.show:
        gated, unresolved, ungated = classify_sources(root, tracked_rust_sources(root))
        for src in unresolved:
            print(f"(unresolved) {src}")
        for src in ungated:
            print(f"(ungated) {src}")
        for name, files in gated.items():
            print(f"{name}: {len(files)} gated file(s)")
        makefile = (root / MAKEFILE).read_text(encoding="utf-8")
        selected = variable(makefile, VARIABLE) or []
        print(f"{VARIABLE}: {', '.join(selected) or '(empty)'}")
        return 0

    problems = check(root)
    if problems:
        print("\n".join(f"postgres-ci-wiring: {p}" for p in problems), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
