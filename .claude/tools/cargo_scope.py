#!/usr/bin/env python3
"""Crate-scoped ``clippy`` / ``rustdoc`` hooks — lint only the crates a file
list touches.

Both hooks used to be ``pass_filenames: false`` with ``types: rust``, so any
Rust file in the set ran a workspace ``cargo clippy --all-targets`` and a
workspace ``cargo doc``. ``make lint-abridged`` (``lint_paths.py --changed``)
narrowed the *file* list but not the *crate* set: a one-line edit in ``util/``
still paid both full builds, on the shared cargo pool's debug lock, several
times per PR in every session. A hand count of that lock's queue on
2026-10-26 found nine of fourteen entries were these two commands.

So the hooks now take the file list (``pass_filenames`` defaults on) through
this tool, which maps each path to its owning package and runs::

    cargo clippy -p <crate>… --all-targets -- -D warnings
    env RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps -p <crate>…

Three rules keep it from ever checking *less* than the crates it was given:

* **Every file ⇒ today's command, byte for byte.** When the Rust paths cover
  every git-tracked ``.rs`` file — the whole-tree ``make lint`` and CI's
  ``--all-files`` — the argv is the pre-scoping one, so the full sweep and the
  merge queue are unchanged.
* **A mapping gap degrades to the full sweep.** A Rust file with no owning
  ``[package]`` (a workspace-level file), or whose package is not a workspace
  member, maps to every crate.
* **No Rust file ⇒ nothing runs.** ``types: rust`` already arranges that; the
  tool exits 0 on an empty set too, for a caller that is not pre-commit.

The hooks set ``require_serial: true``: pre-commit otherwise splits a file
list into parallel chunks, which would race two cargo builds on one lock. The
whole tree's Rust paths fit one invocation, so serial means one cargo run.

The accepted trade: a change to one crate's public API that breaks a
**dependent** crate's build or doc links is not seen locally. The merge
queue's full run catches it — the trade ``--changed`` was designed to make
(``docs/ci.md`` → "Local lint versus CI").

``packages_for`` is the reusable half — path list in, crate names out, ``None``
meaning the whole workspace — and ``--print`` shows the command without
running it.

Stdlib only. This is a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import glob
import os
import shlex
import subprocess
import sys
import tomllib

# The pre-scoping hook commands, kept verbatim so the whole-tree form stays
# byte-identical to what `make lint` and CI ran before this tool existed. The
# root manifest is virtual, so a bare `cargo clippy` already covers every
# member; `cargo doc` needs `--workspace` spelled out.
CLIPPY_LINTS = ["--", "-D", "warnings"]
WORKSPACE_CMDS = {
    "clippy": ["cargo", "clippy", "--all-targets", *CLIPPY_LINTS],
    "doc": ["cargo", "doc", "--no-deps", "--workspace"],
}

# Carried in the environment rather than the argv (pre-commit's `env` prefix
# in the old entry), and written without a space for the same shlex reason.
DOC_ENV = {"RUSTDOCFLAGS": "-Dwarnings"}


def _manifest(path: str) -> dict | None:
    """Parse a ``Cargo.toml``, or ``None`` when there is none to read."""
    try:
        with open(path, "rb") as handle:
            return tomllib.load(handle)
    except FileNotFoundError:
        return None


def owning_package(path: str, root: str) -> tuple[str, str] | None:
    """Return ``(package_dir, name)`` for the nearest ``Cargo.toml`` with a
    ``[package]`` at or above ``path``, stopping at ``root``.

    ``package_dir`` is relative to ``root``. ``None`` means no package owns the
    file — the root manifest is a virtual one, so a workspace-level file lands
    here.
    """
    rel = os.path.normpath(path)
    current = os.path.dirname(rel)
    while True:
        manifest = _manifest(os.path.join(root, current, "Cargo.toml"))
        package = (manifest or {}).get("package")
        if package and "name" in package:
            return os.path.normpath(current), package["name"]
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
    """Every git-tracked ``.rs`` path under ``root``."""
    raw = subprocess.run(
        ["git", "ls-files", "-z", "--", "*.rs"],
        capture_output=True,
        text=True,
        check=True,
        cwd=root,
    ).stdout
    return {os.path.normpath(p) for p in raw.split("\0") if p}


def packages_for(
    files: list[str], root: str, tracked: set[str] | None = None
) -> list[str] | None:
    """Map ``files`` to the sorted crate names that own them.

    Returns ``[]`` when no Rust file is in the list (nothing to run) and
    ``None`` when the answer is the whole workspace: the Rust paths cover every
    tracked ``.rs`` file, or one of them has no owning member package.
    ``tracked`` overrides the ``git ls-files`` read, for tests.
    """
    rust = {os.path.normpath(f) for f in files if f.endswith(".rs")}
    if not rust:
        return []
    if tracked is None:
        tracked = tracked_rust_files(root)
    if tracked <= rust:
        return None
    members = workspace_members(root)
    names: set[str] = set()
    for path in rust:
        owner = owning_package(path, root)
        if owner is None or owner[0] not in members:
            return None
        names.add(owner[1])
    return sorted(names)


def build_cmd(hook: str, packages: list[str] | None) -> list[str]:
    """The cargo argv for ``hook`` over ``packages`` (``None`` = workspace)."""
    if packages is None:
        return list(WORKSPACE_CMDS[hook])
    selectors = [arg for name in packages for arg in ("-p", name)]
    if hook == "clippy":
        return ["cargo", "clippy", *selectors, "--all-targets", *CLIPPY_LINTS]
    return ["cargo", "doc", "--no-deps", *selectors]


def build_env(hook: str) -> dict[str, str]:
    """The extra environment ``hook`` runs under."""
    return dict(DOC_ENV) if hook == "doc" else {}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run clippy or rustdoc over only the crates owning FILES "
        "(the whole workspace when FILES is every tracked Rust file).",
    )
    parser.add_argument("hook", choices=sorted(WORKSPACE_CMDS))
    parser.add_argument("files", nargs="*")
    parser.add_argument(
        "--print",
        action="store_true",
        help="print the command instead of running it",
    )
    parser.add_argument(
        "--root",
        default=".",
        help="the workspace root (pre-commit runs hooks from it)",
    )
    args = parser.parse_args(argv)

    packages = packages_for(args.files, args.root)
    if packages == []:
        return 0
    cmd = build_cmd(args.hook, packages)
    extra = build_env(args.hook)
    shown = shlex.join([*(f"{k}={v}" for k, v in extra.items()), *cmd])
    if args.print:
        print(shown)
        return 0
    print(f"cargo_scope: {shown}", file=sys.stderr)
    return subprocess.run(cmd, cwd=args.root, env={**os.environ, **extra}).returncode


if __name__ == "__main__":
    sys.exit(main())
