#!/usr/bin/env python3
"""Crate-scoped ``clippy`` / ``rustdoc`` hooks — lint only the crates a file
list touches.

Both hooks used to be ``pass_filenames: false`` with ``types: rust``, so any
Rust file in the set ran a workspace ``cargo clippy --all-targets`` and a
workspace ``cargo doc``. ``lint_paths.py --changed`` narrowed the *file* list
but not the *crate* set: a one-line edit in ``util/`` still paid both full
builds, on the shared cargo pool's debug lock, several times per PR in every
session. A hand count of that lock's queue on 2026-10-26 found nine of
fourteen entries were these two commands.

So the hooks take the file list (``pass_filenames`` defaults on) through this
tool, which maps each path to its owning package and runs::

    cargo clippy -p <crate>… --features <crate>/<feature>,… --all-targets \
        -- -D warnings
    RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps -p <crate>… --features …

Four rules keep it from ever checking *less* of a crate than the workspace
build does:

* **Every file ⇒ the workspace command** (``WORKSPACE_CMDS``), byte for byte.
  When the Rust paths cover every tracked ``.rs`` file still on disk — the
  whole-tree ``make lint`` and CI's ``--all-files`` — the full sweep and the
  merge queue run exactly what they ran before the hooks were scoped.
* **The workspace's features, not the crate's defaults.** Cargo unifies
  features only across the packages selected, so a bare ``-p dropset-feeds``
  builds with ``default = []`` and never compiles its ``http`` / ``store`` /
  ``rpc`` / ``stream`` modules — the code an edit there most likely touched.
  Each selected crate gets the features the workspace resolution enables on it
  (``cargo metadata``'s ``resolve`` graph), passed as ``--features``. That
  graph unifies across every target and dev-dependency — exactly clippy's
  ``--all-targets`` set, and a superset of the workspace doc build's (a
  dev-only ``test-support`` feature, say), so rustdoc errs toward checking more.
* **A gap degrades to the full sweep.** A Rust file with no owning
  ``[package]`` (a workspace-level file), one outside a workspace member, or a
  ``cargo metadata`` that fails, maps to the whole workspace.
* **No Rust file ⇒ nothing runs.** ``types: rust`` already arranges that; the
  tool exits 0 on an empty set too, for a caller that is not pre-commit.

The hooks set ``require_serial: true``: pre-commit otherwise splits a file list
into parallel chunks, which would race two cargo builds on one lock. Serial
chunks are cut only at pre-commit's argv cap (128 KiB); the whole tree's Rust
paths are a small fraction of it, so the full sweep is one cargo run. Should
the tree ever outgrow the cap, each chunk runs scoped and their union still
covers every crate — only the byte-identical argv is lost.

The accepted trade: a change to one crate's public API that breaks a
**dependent** crate's build or doc links is not seen locally. The merge
queue's full run catches it — the trade ``--changed`` was designed to make
(``docs/ci.md`` → "Local lint versus CI").

The reusable half is ``packages_for`` (path list in, crate names out, or
``WORKSPACE``) with ``resolved_features`` and ``build_cmd``; ``--print`` shows
the command without running it.

``test`` and ``affected`` — ``make test-abridged``
--------------------------------------------------

``test`` runs ``cargo test`` over the **affected set**: the crates owning the
changed files plus every workspace crate depending on them, transitively, over
every dependency kind (``affected`` prints that set as ``-p`` flags, or
``--workspace``). Linting a crate needs only that crate; testing it does not,
since a dependent's tests exercise the changed API. The graph comes from the
same ``cargo metadata`` call as the features, so nothing is guessed, and the
feature rule above holds here too: a bare ``cargo test -p dropset-feeds``
silently drops its feature-gated tests.

What seeds the set is wider than Rust source, because a suite's result also
moves with the data it reads: a path ``review_diff.rust_is_reachable`` accepts
seeds its owning crate (an asm file, a migration, a manifest), and a fixture a
Rust file reads (``review_diff.rust_test_fixtures``) seeds the reader's crate.
A reachable path no member owns (``Cargo.lock``, ``rust-toolchain.toml``)
seeds the whole workspace. Anything else seeds nothing, so a docs-only or
frontend-only branch runs no cargo test and never waits on the pool's lock.

Two refinements the graph cannot see live in ``ARTIFACT_CONSUMERS``: the
program crate in the set rebuilds the program first (``make program``) and
pulls in every crate whose tests load the ``.so``. The whole workspace runs
``make program`` and then ``cargo test``, byte-identical to ``make test``.

``--changed`` takes the branch's files the way ``lint_paths.py --changed``
does (the merge-base diff with ``origin/main``, plus untracked), deletions
kept: removing a file changes its crate.

Stdlib only. This is a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import shlex
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import NamedTuple

import lint_paths
import review_diff

# `packages_for`'s answer when the scope is the whole workspace. Named so a
# caller cannot fold it into the empty list, which means the opposite.
WORKSPACE = None

# The workspace commands, verbatim from the hook entries they replaced, so the
# whole-tree form is byte-identical to the pre-scoping full sweep. The root
# manifest is virtual, so a bare `cargo clippy` already covers every member;
# `cargo doc` needs `--workspace` spelled out.
CLIPPY_LINTS = ["--", "-D", "warnings"]
WORKSPACE_CMDS = {
    "clippy": ["cargo", "clippy", "--all-targets", *CLIPPY_LINTS],
    "doc": ["cargo", "doc", "--no-deps", "--workspace"],
    # `make test`'s recipe, after its `program` prerequisite.
    "test": ["cargo", "test"],
}

# Set in the environment rather than the argv. Kept as the old entry spelled
# it, without a space, so the full sweep's effective flags are unchanged.
DOC_ENV = {"RUSTDOCFLAGS": "-Dwarnings"}

# Crate → the crates whose tests load the artifact its build produces, a
# runtime file read the dependency graph cannot see. A producer in the
# affected set is built first (`ARTIFACT_BUILDS`) and pulls its consumers in.
# Only the program crate's own `tests/` read `target/deploy/dropset.so` today
# (the litesvm suites and the SDK conformance replay); a crate that starts
# reading it joins the tuple.
PROGRAM_CRATE = "dropset"
ARTIFACT_CONSUMERS = {PROGRAM_CRATE: ("dropset",)}
ARTIFACT_BUILDS = {PROGRAM_CRATE: ["make", "program"]}


class Package(NamedTuple):
    """A package's directory, relative to the workspace root, and its name."""

    dir: str
    name: str


def repo_root() -> str:
    """Return the working tree's top-level directory, the default ``--root``.

    pre-commit runs hooks from the root, but a hand-run ``--print`` may not, so
    the root is pinned here rather than trusting the caller's cwd.
    """
    return subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def _manifest(path: str) -> dict | None:
    """Parse a ``Cargo.toml``, or ``None`` when there is none to read."""
    try:
        with open(path, "rb") as handle:
            return tomllib.load(handle)
    except FileNotFoundError:
        return None


def owning_package(path: str, root: str) -> Package | None:
    """Return the nearest package whose ``Cargo.toml`` has a ``[package]``, at
    or above ``path`` (relative to ``root``), stopping at ``root``.

    ``None`` means no package owns the file — the root manifest is a virtual
    one, so a workspace-level file lands here, as does a path outside ``root``.
    """
    rel = os.path.normpath(path)
    if os.path.isabs(rel) or rel.startswith(os.pardir):
        return None
    current = os.path.dirname(rel)
    while True:
        manifest = _manifest(os.path.join(root, current, "Cargo.toml"))
        package = (manifest or {}).get("package")
        if package and "name" in package:
            return Package(os.path.normpath(current), package["name"])
        if current in ("", "."):
            return None
        current = os.path.dirname(current)


def workspace_members(root: str) -> set[str]:
    """The root manifest's ``[workspace] members``, globs expanded, as
    normalized paths relative to ``root``."""
    manifest = _manifest(os.path.join(root, "Cargo.toml")) or {}
    members = manifest.get("workspace", {}).get("members", [])
    found: set[str] = set()
    for member in members:
        matches = glob.glob(member, root_dir=root) or [member]
        found.update(os.path.normpath(m) for m in matches)
    return found


def tracked_rust_files(root: str) -> set[str]:
    """Every git-tracked ``.rs`` path under ``root`` that is still on disk.

    A tracked file deleted from the worktree never reaches the hook (pre-commit
    and ``lint_paths.py`` both drop it), so counting it would stop the
    whole-tree run from being recognized as one.
    """
    raw = subprocess.run(
        ["git", "ls-files", "-z", "--", "*.rs"],
        capture_output=True,
        text=True,
        check=True,
        cwd=root,
    ).stdout
    return {
        os.path.normpath(p)
        for p in raw.split("\0")
        if p and os.path.lexists(os.path.join(root, p))
    }


def packages_for(
    files: list[str], root: str, tracked: set[str] | None = None
) -> list[str] | None:
    """Map ``files`` (relative to ``root``) to the sorted crate names owning
    them.

    Returns ``[]`` when no Rust file is in the list (nothing to run) and
    ``WORKSPACE`` when the answer is the whole workspace: the Rust paths cover
    every tracked ``.rs`` file, or one of them has no owning member package.
    ``tracked`` overrides the ``git ls-files`` read, for tests.
    """
    rust = {os.path.normpath(f) for f in files if f.endswith(".rs")}
    if not rust:
        return []
    if tracked is None:
        tracked = tracked_rust_files(root)
    if tracked <= rust:
        return WORKSPACE
    members = workspace_members(root)
    names: set[str] = set()
    for path in rust:
        owner = owning_package(path, root)
        if owner is None or owner.dir not in members:
            return WORKSPACE
        names.add(owner.name)
    return sorted(names)


def parse_resolved_features(metadata: dict) -> dict[str, list[str]]:
    """Map each workspace member's name to the features the workspace
    resolution enables on it, ``default`` dropped (``-p`` already implies it).

    Reads ``cargo metadata --format-version 1`` output: ``resolve.nodes`` holds
    the unified feature set per package id.
    """
    members = set(metadata["workspace_members"])
    names = {p["id"]: p["name"] for p in metadata["packages"] if p["id"] in members}
    return {
        names[node["id"]]: sorted(f for f in node["features"] if f != "default")
        for node in metadata["resolve"]["nodes"]
        if node["id"] in names
    }


def cargo_metadata(root: str) -> dict | None:
    """A live ``cargo metadata --format-version 1``, or ``None`` when cargo
    cannot answer — the caller then falls back to the workspace."""
    proc = subprocess.run(
        ["cargo", "metadata", "--format-version", "1"],
        capture_output=True,
        text=True,
        cwd=root,
    )
    if proc.returncode != 0:
        return None
    return json.loads(proc.stdout)


def resolved_features(root: str) -> dict[str, list[str]] | None:
    """``parse_resolved_features`` over a live ``cargo metadata``, or ``None``
    when cargo cannot answer."""
    metadata = cargo_metadata(root)
    return None if metadata is None else parse_resolved_features(metadata)


def reverse_dependents(metadata: dict) -> dict[str, set[str]]:
    """Map each workspace member's name to the members depending on it
    directly, over every dependency kind (normal, dev, build).

    Reads ``resolve.nodes[].deps``; an edge to a non-member is dropped, since
    only a member's tests are ever run.
    """
    members = set(metadata["workspace_members"])
    names = {p["id"]: p["name"] for p in metadata["packages"] if p["id"] in members}
    dependents: dict[str, set[str]] = {name: set() for name in names.values()}
    for node in metadata["resolve"]["nodes"]:
        if node["id"] not in names:
            continue
        for dep in node.get("deps", []):
            if dep["pkg"] in names:
                dependents[names[dep["pkg"]]].add(names[node["id"]])
    return dependents


def affected_closure(seeds: list[str], metadata: dict) -> list[str] | None:
    """``seeds`` plus every member depending on one of them, transitively,
    with each artifact producer's consumers pulled in — sorted, or
    ``WORKSPACE`` when that is every member."""
    dependents = reverse_dependents(metadata)
    found: set[str] = set()
    queue = list(seeds)
    while queue:
        name = queue.pop()
        if name in found:
            continue
        found.add(name)
        queue.extend(dependents.get(name, ()))
        queue.extend(ARTIFACT_CONSUMERS.get(name, ()))
    if found >= set(dependents):
        return WORKSPACE
    return sorted(found)


def test_seeds(
    files: list[str],
    root: str,
    tracked: set[str] | None = None,
    fixtures: dict[str, list[str]] | None = None,
) -> list[str] | None:
    """The crates ``files`` change for a test run: sorted names, ``[]`` for
    nothing to test, or ``WORKSPACE``.

    A path ``review_diff.rust_is_reachable`` accepts seeds its owning member,
    and the whole workspace when no member owns it; a fixture some Rust file
    reads seeds the reader's crate. Rust files covering every tracked one are
    the workspace, as in ``packages_for``. ``tracked`` and ``fixtures``
    override the tree reads, for tests.
    """
    paths = sorted({os.path.normpath(f) for f in files})
    rust = {p for p in paths if p.endswith(".rs")}
    if rust:
        if tracked is None:
            tracked = tracked_rust_files(root)
        if tracked <= rust:
            return WORKSPACE
    members = workspace_members(root)
    names: set[str] = set()

    def owner_name(path: str) -> str | None:
        owner = owning_package(path, root)
        return owner.name if owner and owner.dir in members else None

    for path in paths:
        # Scanned once, and only for a non-Rust path: the fixtures in this
        # tree are data files, never another crate's source.
        if fixtures is None and not path.endswith(".rs"):
            fixtures = review_diff.rust_test_fixtures(
                sorted(tracked_rust_files(root)), Path(root)
            )
        if review_diff.rust_is_reachable([path]):
            own = owner_name(path)
            if own is None:
                return WORKSPACE
            names.add(own)
        for key, readers in (fixtures or {}).items():
            if path != key and not path.startswith(key + "/"):
                continue
            for reader in readers:
                name = owner_name(reader)
                if name is None:
                    return WORKSPACE
                names.add(name)
    return sorted(names)


def affected(
    files: list[str],
    root: str,
    metadata: dict | None,
    tracked: set[str] | None = None,
    fixtures: dict[str, list[str]] | None = None,
) -> list[str] | None:
    """The affected set for ``files``: ``test_seeds`` widened by
    ``affected_closure``. ``WORKSPACE`` when ``metadata`` is ``None``, since
    no graph means no safe narrowing."""
    seeds = test_seeds(files, root, tracked, fixtures)
    if not seeds:
        return seeds
    if metadata is None:
        return WORKSPACE
    return affected_closure(seeds, metadata)


def artifact_builds(packages: list[str] | None) -> list[list[str]]:
    """The artifact builds a test run over ``packages`` needs first."""
    return [
        cmd
        for crate, cmd in ARTIFACT_BUILDS.items()
        if packages is WORKSPACE or crate in packages
    ]


def scope_flags(packages: list[str] | None) -> str:
    """``affected``'s output: the ``-p`` list, ``--workspace``, or empty."""
    if packages is WORKSPACE:
        return "--workspace"
    return shlex.join(arg for name in packages for arg in ("-p", name))


def changed_paths(root: str) -> list[str]:
    """``lint_paths.changed_files`` with deleted paths kept: the merge-base
    diff with ``origin/main`` plus untracked-not-ignored paths."""
    base = lint_paths.merge_base(root, lint_paths.DEFAULT_BASE)
    tracked = subprocess.run(
        ["git", "diff", "--name-only", "-z", base, "--"],
        capture_output=True,
        text=True,
        check=True,
        cwd=root,
    ).stdout
    untracked = subprocess.run(
        lint_paths._LS_OTHERS,
        capture_output=True,
        text=True,
        check=True,
        cwd=root,
    ).stdout
    return lint_paths.parse_ls_files(tracked + "\0" + untracked)


def build_cmd(
    hook: str,
    packages: list[str] | None,
    features: dict[str, list[str]] | None = None,
) -> list[str]:
    """The cargo argv for ``hook`` over ``packages`` (``WORKSPACE`` = all),
    each selected crate built with its entry in ``features``."""
    if packages is WORKSPACE:
        return list(WORKSPACE_CMDS[hook])
    if not packages:
        raise ValueError("no packages: nothing to run, not the workspace")
    selected = [arg for name in packages for arg in ("-p", name)]
    enabled = [
        f"{name}/{feature}"
        for name in packages
        for feature in (features or {}).get(name, [])
    ]
    if enabled:
        selected += ["--features", ",".join(enabled)]
    if hook == "clippy":
        return ["cargo", "clippy", *selected, "--all-targets", *CLIPPY_LINTS]
    if hook == "test":
        return ["cargo", "test", *selected]
    return ["cargo", "doc", "--no-deps", *selected]


def build_env(hook: str) -> dict[str, str]:
    """The extra environment ``hook`` runs under."""
    return dict(DOC_ENV) if hook == "doc" else {}


def run_tests(hook: str, files: list[str], root: str, show: bool) -> int:
    """``affected`` prints the set; ``test`` builds what it needs and runs
    ``cargo test`` over it (``show`` prints the commands instead)."""
    seeds = test_seeds(files, root)
    if seeds == []:
        if hook == "test":
            print(
                "cargo_scope: no Rust-reachable change, nothing to test",
                file=sys.stderr,
            )
        return 0
    # Asked only once there is something to test, so a docs-only branch never
    # waits on cargo at all.
    metadata = None if seeds is WORKSPACE else cargo_metadata(root)
    packages = WORKSPACE if metadata is None else affected_closure(seeds, metadata)
    if hook == "affected":
        print(scope_flags(packages))
        return 0
    features = None if packages is WORKSPACE else parse_resolved_features(metadata)
    for cmd in [*artifact_builds(packages), build_cmd("test", packages, features)]:
        shown = shlex.join(cmd)
        if show:
            print(shown)
            continue
        print(f"cargo_scope: {shown}", file=sys.stderr)
        code = subprocess.run(cmd, cwd=root).returncode
        if code != 0:
            return code
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run clippy or rustdoc over only the crates owning FILES "
        "(the whole workspace when FILES is every tracked Rust file); or run "
        "`cargo test` over, or print, the crates FILES affect.",
    )
    parser.add_argument("hook", choices=sorted([*WORKSPACE_CMDS, "affected"]))
    parser.add_argument("files", nargs="*")
    parser.add_argument(
        "--changed",
        action="store_true",
        help="add this branch's files: the merge-base diff with origin/main, "
        "plus untracked",
    )
    parser.add_argument(
        "--print",
        action="store_true",
        help="print the command instead of running it",
    )
    parser.add_argument(
        "--root",
        help="the workspace root (default: the git top-level directory)",
    )
    args = parser.parse_args(argv)
    root = args.root or repo_root()
    # FILES are relative to the root, as pre-commit passes them; an absolute
    # path is brought into that form (one outside the root then maps to
    # nothing, so the whole workspace).
    files = [os.path.relpath(f, root) if os.path.isabs(f) else f for f in args.files]
    if args.changed:
        files += changed_paths(root)
    if args.hook in ("test", "affected"):
        return run_tests(args.hook, files, root, args.print)

    packages = packages_for(files, root)
    if packages == []:
        return 0
    features = None
    if packages is not WORKSPACE:
        features = resolved_features(root)
        if features is None:
            packages = WORKSPACE
    cmd = build_cmd(args.hook, packages, features)
    extra = build_env(args.hook)
    shown = shlex.join([*(f"{k}={v}" for k, v in extra.items()), *cmd])
    if args.print:
        print(shown)
        return 0
    print(f"cargo_scope: {shown}", file=sys.stderr)
    return subprocess.run(cmd, cwd=root, env={**os.environ, **extra}).returncode


if __name__ == "__main__":
    sys.exit(main())
