#!/usr/bin/env python3
# cspell:word followlinks
"""Docker build-context hygiene: the root ``.dockerignore`` stays honest.

Every Rust service in ``infra/localnet/docker-compose.yml`` builds with its
context set to the **repo root** (``context: '../..'``) and its Dockerfile does
``COPY . .``. With no ignore file, that ships the entire checkout to the Docker
daemon on every build. Measured on the base checkout before this guard landed:

===========================  =========
tree                         size
===========================  =========
``.claude/worktrees``          57 GB
``target``                     12 GB
``node_modules``              1.3 GB
``frontend`` (minus the two)  594 MB
``.git``                       69 MB
===========================  =========

— about **71 GB** of context transfer per build. An operator hit it live and
watched a single collector build climb past 40 GB before killing it.

**Why the biggest tree is the one a checkout cannot see.** ``.claude/worktrees``
holds one full checkout per agent worktree, each with its own ``target/`` and
``node_modules``, and it is excluded only by ``.git/info/exclude`` — a
**user-local, uncommitted** file. So a fresh clone has nothing keeping it out of
a build context, and no committed ignore file mentioned it. That asymmetry is
the whole reason this guard names its required patterns explicitly rather than
deferring to git: ``.dockerignore`` cannot inherit what git itself only knows
locally.

Two checks, because the failure has two shapes:

1. **Presence** — the ignore file exists and still names every known-fat tree.
   A pattern deleted or renamed away is the silent regression; a build then
   works, just slowly, so nothing fails until someone is watching a progress
   bar. Checked against a literal required set, so the diff names the missing
   pattern.
2. **Ceiling** — the effective context measures under an order-of-magnitude
   bound. This catches the case presence cannot: a *new* fat tree nobody
   thought to add a pattern for. The bound is deliberately loose (see
   ``CEILING_BYTES``) — it is a tripwire for a runaway, not a budget.

The measurement prunes ignored directories as it walks, so it never descends
into the trees it is excluding; measuring the 71 GB "before" costs the same
walk as the 130 MB "after".

**Not a faithful copy of Docker's matcher.** It implements the subset this
repo's ignore file uses — comments, ``!`` negation with last-match-wins,
``*``/``?``/``**`` globbing, and an excluded directory carrying its whole
subtree with it. It is a guard on our own file, not a general tool; a pattern
form we do not use is out of scope by design rather than by oversight.

Stdlib only. This is a Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling").
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from typing import NamedTuple

# Patterns the root ignore file must carry, each with the tree it bounds and
# why that tree is not a build input. Keyed by the literal pattern line so a
# failure can name exactly what to add back.
#
# Spelled `**/<name>` rather than `<name>` wherever the tree recurs: Docker
# cleans a pattern with `filepath.Clean`, so a bare `target` anchors at the
# context root and would miss `bots/maker-bot/target` and every worktree's own.
REQUIRED_PATTERNS: dict[str, str] = {
    "**/target": "Rust build output — 12 GB in the base checkout",
    ".claude/worktrees": (
        "agent worktrees — one full checkout each, with its own target/ "
        "and node_modules (57 GB, and git-excluded only locally)"
    ),
    "**/.git": "git metadata; no build script reads it",
    "**/node_modules": "pnpm workspace deps, hoisted to the root (1.3 GB)",
    "**/.next": "Next.js build output",
    "frontend/public": "generated in full by the predev/prebuild hooks",
}

# Order-of-magnitude ceiling on the effective context, per the task's "assert
# an order-of-magnitude ceiling rather than an exact number". The context
# measured ~130 MB with the ignore file in place, so this leaves roughly 4x of
# headroom: a new crate, a fixture set, or a vendored dependency lands without
# anyone touching this file, and only a genuinely runaway tree trips it.
#
# Raising this is a legitimate edit — but read the measurement first
# (`--measure`), because the cheap fix is usually a missing pattern, not a
# bigger bound.
CEILING_BYTES = 512 * 1024 * 1024

_ROOT_MARKERS = ("Cargo.toml", "pnpm-workspace.yaml")


class Rule(NamedTuple):
    """One parsed ignore line: a compiled matcher plus its polarity."""

    negated: bool
    pattern: str
    regex: re.Pattern[str]


class Measurement(NamedTuple):
    """What one context walk found."""

    files: int
    total_bytes: int
    pruned: list[tuple[str, int]]

    def render(self) -> str:
        lines = [
            f"context: {human(self.total_bytes)} across {self.files} files",
        ]
        if self.pruned:
            lines.append(f"pruned {len(self.pruned)} directory tree(s):")
            lines.extend(f"  {path}" for path, _ in sorted(self.pruned))
        return "\n".join(lines)


def human(size: int) -> str:
    """Render a byte count in the largest unit that keeps it above 1."""
    value = float(size)
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if value < 1024 or unit == "TB":
            return f"{value:.1f} {unit}" if unit != "B" else f"{int(value)} B"
        value /= 1024
    raise AssertionError("unreachable")


def _translate(pattern: str) -> re.Pattern[str]:
    """Compile one cleaned ignore pattern to a full-match regex.

    ``**`` spans separators, ``*`` and ``?`` do not — matching Docker's
    matcher rather than Python's ``fnmatch``, which lets ``*`` cross a ``/``
    and would make ``**/target`` and ``*/target`` synonyms.
    """
    out = ["(?s:"]
    index = 0
    length = len(pattern)
    while index < length:
        char = pattern[index]
        if char == "*":
            if pattern.startswith("**", index):
                index += 2
                # A leading `**/` must also match at depth zero, so that
                # `**/target` covers a root-level `target` as well as a nested
                # one. Docker treats the two the same way.
                if pattern.startswith("/", index):
                    index += 1
                    out.append("(?:.*/)?")
                else:
                    out.append(".*")
                continue
            out.append("[^/]*")
        elif char == "?":
            out.append("[^/]")
        else:
            out.append(re.escape(char))
        index += 1
    out.append(")\\Z")
    return re.compile("".join(out))


def clean_pattern(raw: str) -> str:
    """Normalize a pattern the way ``filepath.Clean`` does before matching."""
    pattern = raw.strip()
    while pattern.startswith("./"):
        pattern = pattern[2:]
    # A trailing separator is dropped, so `target/` and `target` are one
    # pattern. Docker has no directory-only pattern form.
    while pattern.endswith("/") and pattern != "/":
        pattern = pattern[:-1]
    return pattern


def parse_dockerignore(text: str) -> list[Rule]:
    """Parse ignore-file text into rules, in file order.

    Blank lines and ``#`` comments are dropped. A leading ``!`` marks a
    negation, which un-ignores a path an earlier rule matched.
    """
    rules: list[Rule] = []
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        negated = line.startswith("!")
        if negated:
            line = line[1:]
        pattern = clean_pattern(line)
        if not pattern:
            continue
        rules.append(Rule(negated, pattern, _translate(pattern)))
    return rules


def _decide(relpath: str, rules: list[Rule], inherited: bool) -> bool:
    """Apply every rule to one path component-path, last match winning.

    Docker evaluates all rules and the final match decides, which is what
    makes a ``!`` re-include work regardless of the order of the rule it
    overrides. With no rule matching at all, the state passes through.
    """
    ignored = inherited
    for rule in rules:
        if rule.regex.match(relpath):
            ignored = not rule.negated
    return ignored


def is_ignored(relpath: str, rules: list[Rule]) -> bool:
    """Whether ``relpath`` is excluded from the context.

    **Ancestors count.** Excluding a directory excludes everything under it,
    so ``frontend/public`` keeps out ``frontend/public/flags/us.svg`` even
    though no pattern names that file. The decision is therefore made by
    walking the path shallowest-first and letting each level inherit the
    previous one's verdict, rather than matching the full path alone — which
    would have reported that file as included and made this function disagree
    with both Docker and ``measure``'s own pruning.

    Inheriting rather than short-circuiting is what keeps a re-include
    working at depth: a ``!frontend/public/keep.svg`` still gets its say after
    its parent directory has been excluded.
    """
    parts = [part for part in relpath.split("/") if part]
    ignored = False
    for depth in range(1, len(parts) + 1):
        ignored = _decide("/".join(parts[:depth]), rules, ignored)
    return ignored


def measure(root: str, rules: list[Rule]) -> Measurement:
    """Walk ``root`` and total what a build would actually receive.

    Directories matching an ignore rule are pruned rather than descended, so
    the walk never pays for the trees it is excluding. A pruned directory is
    recorded for the report — that list is the useful half of ``--measure``,
    since it shows which patterns are doing the work.

    A negation could in principle re-include something under a pruned
    directory. This repo's ignore file has no ``!`` rules, so pruning stays
    exact here; a future negation would need this to stop pruning that subtree.
    """
    files = 0
    total = 0
    pruned: list[tuple[str, int]] = []
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
        rel_dir = os.path.relpath(dirpath, root)
        prefix = "" if rel_dir == "." else rel_dir + "/"

        keep: list[str] = []
        for name in dirnames:
            if is_ignored(prefix + name, rules):
                pruned.append((prefix + name, 0))
            else:
                keep.append(name)
        # Mutated in place so os.walk skips the pruned trees entirely.
        dirnames[:] = keep

        for name in filenames:
            rel = prefix + name
            if is_ignored(rel, rules):
                continue
            full = os.path.join(dirpath, name)
            try:
                # lstat, not stat: a symlink contributes its own small entry
                # rather than whatever it points at, and a dangling one (a
                # worktree's operator-file link to a base repo that moved)
                # must not abort the walk.
                total += os.lstat(full).st_size
            except OSError:
                continue
            files += 1
    return Measurement(files, total, pruned)


def find_root(start: str) -> str:
    """Walk up from ``start`` to the checkout root, by marker file."""
    current = os.path.abspath(start)
    while True:
        if all(
            os.path.exists(os.path.join(current, marker)) for marker in _ROOT_MARKERS
        ):
            return current
        parent = os.path.dirname(current)
        if parent == current:
            raise SystemExit(
                f"docker-context: no checkout root above {start} "
                f"(looked for {' + '.join(_ROOT_MARKERS)})"
            )
        current = parent


def missing_patterns(text: str) -> list[str]:
    """Required patterns the ignore file does not carry."""
    present = {rule.pattern for rule in parse_dockerignore(text) if not rule.negated}
    return [
        pattern
        for pattern in REQUIRED_PATTERNS
        if clean_pattern(pattern) not in present
    ]


def check(root: str) -> tuple[int, list[str]]:
    """Run both checks. Returns an exit code and the lines to report."""
    path = os.path.join(root, ".dockerignore")
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except FileNotFoundError:
        return 1, [
            "docker-context: no .dockerignore at the context root "
            f"({os.path.relpath(path, root)}).",
            "",
            "Every Rust service in infra/localnet/docker-compose.yml builds "
            "with context '../..' and COPY . ., so without this file each "
            "build ships the whole checkout (~71 GB) to the daemon.",
            "",
            "Required patterns:",
            *(f"  {name}  # {why}" for name, why in REQUIRED_PATTERNS.items()),
        ]

    problems: list[str] = []

    missing = missing_patterns(text)
    if missing:
        problems.append(
            f"docker-context: .dockerignore is missing {len(missing)} "
            "required pattern(s):"
        )
        problems.extend(f"  {name}  # {REQUIRED_PATTERNS[name]}" for name in missing)
        problems.append("")
        problems.append(
            "These trees are not build inputs and are large enough to "
            "dominate the context transfer. Add the pattern back, or update "
            "REQUIRED_PATTERNS in .claude/tools/docker_context.py if the "
            "tree genuinely moved."
        )

    result = measure(root, parse_dockerignore(text))
    if result.total_bytes > CEILING_BYTES:
        if problems:
            problems.append("")
        problems.extend(
            [
                f"docker-context: effective context is "
                f"{human(result.total_bytes)}, over the "
                f"{human(CEILING_BYTES)} ceiling.",
                "",
                "A tree this large is almost never meant to reach the "
                "daemon. Run `--measure` to see what survives the ignore "
                "file, add a pattern for it, and only raise CEILING_BYTES in "
                ".claude/tools/docker_context.py if the growth is genuinely "
                "build input.",
            ]
        )

    if problems:
        return 1, problems
    return 0, [
        f"docker-context: ok — {human(result.total_bytes)} context, "
        f"{len(REQUIRED_PATTERNS)} required patterns present."
    ]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="docker_context.py",
        description=(
            "Guard the root .dockerignore: assert it exists, still names "
            "every known-fat tree, and keeps the effective build context "
            "under an order-of-magnitude ceiling."
        ),
    )
    parser.add_argument(
        "--measure",
        action="store_true",
        help="report the effective context size and the pruned trees",
    )
    parser.add_argument(
        "--no-ignore",
        action="store_true",
        help=(
            "with --measure, ignore the .dockerignore entirely — the 'before' baseline"
        ),
    )
    parser.add_argument(
        "--root",
        default=None,
        help="context root (default: the checkout root above the cwd)",
    )
    parser.add_argument(
        "--ignore-file",
        default=None,
        help=(
            "with --measure, read patterns from here instead of "
            "<root>/.dockerignore — how you measure the 'after' size of a "
            "checkout that does not carry the file yet"
        ),
    )
    args = parser.parse_args(argv)

    root = args.root or find_root(os.getcwd())

    if args.measure:
        rules: list[Rule] = []
        if not args.no_ignore:
            source = args.ignore_file or os.path.join(root, ".dockerignore")
            try:
                with open(source, encoding="utf-8") as handle:
                    rules = parse_dockerignore(handle.read())
            except FileNotFoundError:
                print(
                    f"docker-context: no {source} — measuring the whole tree.",
                    file=sys.stderr,
                )
        print(measure(root, rules).render())
        return 0

    code, lines = check(root)
    stream = sys.stderr if code else sys.stdout
    for line in lines:
        print(line, file=stream)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
