#!/usr/bin/env python3
"""The review tier, its lenses, the lens gates and the gate-integrity tagger.

``review-pr`` step 5 used to spend its longest stretch of prose on decisions
that read only paths and diff shapes: which tier a diff takes, which lenses
the tier implies, whether the security and freshness lenses run, and how a
too-large diff is subdivided. Those are computed here, from the file list and
the parsed review diff, and ``review_diff.py`` puts the result in its verdict.
The session keeps only the judgment: echo the proposed tier, or override it
with a stated reason the summary names.

Three rules the functions below enforce, so no caller has to remember them:

* **The cross-check is in every tier's lens list**, required everywhere. On
  the trivial tier alone it is marked ``ask_to_skip``: the session may OFFER
  to skip it through an ``AskUserQuestion``, never drop it silently.
* **A gate can only add a lens**, never remove one the tier listed.
* **The tagger never decides.** Its hits are facts for the completeness lens
  and the cross-check, and an operator question; nothing here fixes or
  dismisses one.

Pure functions over strings and lists: no git, no filesystem. Standard library
only, deliberately not a Cargo workspace member (``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_review_tier.py``.
"""

from __future__ import annotations

import fnmatch
import re
from collections import Counter
from dataclasses import dataclass, field

# --------------------------------------------------------------------------
# Path patterns. `matches` is the one matcher every list in this module and
# in `review_diff.py` goes through.


def matches(path: str, pattern: str) -> bool:
    """Whether ``path`` matches one path-filter pattern.

    Supports the four shapes the lists use:

    * ``**/seg/**`` — that directory segment at any depth (``**/tests/**``).
      Checked **first**, because it also ends in ``/**`` and would otherwise be
      read as the literal subtree ``**/tests``.
    * ``dir/**`` — a subtree rooted at ``dir``.
    * ``**/*.ext`` — a suffix match at any depth.
    * anything else — a literal path.
    """
    if pattern.startswith("**/") and pattern.endswith("/**"):
        middle = pattern[3:-3]
        return f"/{middle}/" in f"/{path}/"
    if pattern.endswith("/**"):
        prefix = pattern[:-3]
        return path == prefix or path.startswith(prefix + "/")
    if pattern.startswith("**/"):
        return fnmatch.fnmatch(path, pattern[3:]) or fnmatch.fnmatch(path, pattern)
    return path == pattern


def matches_any(path: str, patterns) -> bool:
    """Whether ``path`` matches at least one of ``patterns``."""
    return any(matches(path, p) for p in patterns)


# The trust surface: a program handler or account layout, the SDKs that
# deserialize them, signing and key material, external I/O (feeds, indexer,
# market data, bots), the deploy and CI surface, and the guard hooks, which
# parse the agent's own tool calls and decide what may run. Host tooling (the
# TUI, decks, `util/`) and the rest of `.claude/` are deliberately absent:
# the security lens on tooling was measured as near-zero yield. A docs-path
# file never counts. The gate fails toward adding the lens.
SECURITY_SURFACE = (
    "programs/**",
    "sdk/interface/**",
    "sdk/rs/**",
    "sdk/ts/**",
    "keys/**",
    "bots/**",
    "feeds/**",
    "indexer/**",
    "market-data/**",
    "fair-value/**",
    "db-schema/**",
    "frontend/**",
    "infra/**",
    ".github/workflows/**",
    ".claude/hooks/**",
    # Dependency manifests: a version bump is a supply-chain change, and its
    # lockfile is excluded from the diff, so the manifest is the only trace.
    "**/Cargo.toml",
    "**/package.json",
    "pnpm-workspace.yaml",
    "rust-toolchain.toml",
)

# What the two freshness lenses (doc-freshness, ci-skip-list) check against:
# the convention set, the CI config, and the skills that cite both. A new
# top-level tree is the fourth trigger, computed separately.
FRESHNESS_SURFACE = (
    "CLAUDE.md",
    "docs/conventions/**",
    ".github/**",
    ".claude/skills/**",
)

# Meta-work, matching `CLAUDE.md`'s definition of a `Claude:` issue.
META_SURFACE = (".claude/**", "CLAUDE.md", "docs/**", "cfg/**")

# Infra / ops: build, container, CI and deploy files with no program, SDK or
# app control flow. Docs-path files may ride along.
INFRA_SURFACE = (
    "**/Dockerfile*",
    "**/.dockerignore",
    "**/docker-compose*.yml",
    "**/docker-compose*.yaml",
    "**/compose*.yml",
    "**/compose*.yaml",
    "Makefile",
    "**/Makefile",
    "**/*.mk",
    ".github/**",
    "infra/**",
)

# A config tweak, for the trivial tier's nature test.
CONFIG_SURFACE = (
    "**/*.yml",
    "**/*.yaml",
    "**/*.toml",
    "**/*.json",
    "**/*.ini",
    "**/*.cfg",
    ".github/**",
    "cfg/**",
)

# The entry gate's surfaces that rule out the abridged tier: program code, the
# SDK surface (which holds every generation input), and migrations.
ABRIDGED_EXCLUDED = ("programs/**", "sdk/**", "db-schema/migrations/**")

# The entry gate's small-diff threshold (review-pr's "The entry gate").
ABRIDGED_MAX_FILES = 5
ABRIDGED_MAX_CHANGES = 60

# Small single-crate: "even at a couple hundred lines".
SMALL_CRATE_MAX_CHANGES = 300

# ...and "self-contained": its own tests are a real share of the change, not
# one touched assertion beside 300 lines of new logic.
SMALL_CRATE_TEST_SHARE = 0.25

# Files whose ADDITION declares a new build unit (a crate, a package), which
# the new-tree gate treats like a new top-level tree.
BUILD_MANIFESTS = (
    "**/Cargo.toml",
    "**/package.json",
    "**/pyproject.toml",
)

# Rewiring: in-place value edits only, and bounded.
REWIRING_MAX_CHANGES = 200

# Move and reformat detection need enough lines to mean anything.
SHAPE_MIN_LINES = 20

# Share of removed lines that must reappear as added for a move.
MOVE_SHARE = 0.8

# Past this many diff lines the verdict proposes a path-scoped subdivision for
# the lenses whose question sits inside one tree. Measured, not inherited: of
# the 120 most recent merged PRs (2026-11-21) the changed-line median was
# about 420 and 13 exceeded 1,000 — the PR split band. A review diff runs
# about 1.4x its changed lines in context and headers, so 1,500 diff lines
# fires on roughly that top tenth, the band the slice-size history measured
# at 1.4-3.3x the exemplars.
SUBDIVIDE_DIFF_LINES = 1500

# A top-level group at or above this share of the changes is split one level
# deeper, since handing a lens "nearly everything" scopes nothing.
DOMINANT_SHARE = 0.8

LENS_ORDER = (
    "correctness",
    "ops-correctness",
    "move-fidelity",
    "test-validity",
    "security",
    "style",
    "completeness",
    "comment-accuracy",
    "doc-freshness",
    "ci-skip-list",
    "cross-check",
)

# Each tier's own lenses, before any gate adds one. The cross-check is
# appended to every tier by `lenses_for`.
TIER_LENSES = {
    "trivial": ("correctness",),
    "reduced-infra": ("ops-correctness",),
    "reduced-move": ("move-fidelity",),
    "reduced-reformat": ("move-fidelity",),
    "reduced-rewiring": ("correctness",),
    "reduced-test-only": ("test-validity",),
    "small-single-crate": ("correctness", "completeness"),
    "meta": ("correctness", "completeness"),
    # Security joins the full tier through its gate, like any other tier.
    "full": ("correctness", "style", "completeness"),
}

# Which lenses each gate adds.
GATE_LENSES = {
    "security": ("security",),
    "freshness": ("doc-freshness", "ci-skip-list"),
    "new_tree": ("doc-freshness", "ci-skip-list"),
    "prose_heavy": ("comment-accuracy",),
}

# --------------------------------------------------------------------------
# The parsed diff.


@dataclass
class Hunk:
    """One ``@@`` hunk: its post-image start and length, and its lines."""

    new_start: int
    new_len: int
    # (post-image line number, text without the leading `+`)
    added: list[tuple[int, str]] = field(default_factory=list)
    removed: list[str] = field(default_factory=list)


@dataclass
class FileDiff:
    path: str
    hunks: list[Hunk] = field(default_factory=list)
    # The file is ADDED by the diff (a `new file mode` preamble line).
    new_file: bool = False


_C_ESCAPES = {
    "a": 7,
    "b": 8,
    "t": 9,
    "n": 10,
    "v": 11,
    "f": 12,
    "r": 13,
    '"': 34,
    "\\": 92,
}


def unquote_c_path(text: str) -> str:
    """Undo git's C-style path quoting (the body between the quotes).

    Git escapes ``"``, ``\\`` and control characters by letter, and every
    byte of a non-ASCII name as three octal digits, so the octal runs are
    gathered as bytes and decoded as UTF-8 together. An unknown escape is
    kept as written.
    """
    out = bytearray()
    i = 0
    while i < len(text):
        char = text[i]
        if char != "\\" or i + 1 == len(text):
            out += char.encode()
            i += 1
            continue
        nxt = text[i + 1]
        octal = text[i + 1 : i + 4]
        if len(octal) == 3 and all(c in "01234567" for c in octal):
            out.append(int(octal, 8) & 0xFF)
            i += 4
        elif nxt in _C_ESCAPES:
            out.append(_C_ESCAPES[nxt])
            i += 2
        else:
            out += ("\\" + nxt).encode()
            i += 2
    return out.decode("utf-8", errors="replace")


def diff_header_path(line: str) -> str | None:
    """The destination path from a ``diff --git a/X b/Y`` header, or ``None``.

    Takes the ``b/`` side because that is what the change produced — for a rename
    the ``a/`` side no longer exists. Git keeps a real path on both sides even
    for a deletion, so there is no ``/dev/null`` case here.

    Paths may contain spaces, so the pair is split on the first ``" b/"``; a
    path itself containing that sequence mis-splits, which is accepted. A
    quoted pair (``core.quotePath`` on a non-ASCII path) is split on
    ``" "b/`` and unescaped, so the kind and test-range lookups find the
    file under its real name. A header must never go unrecognized, or its
    hunks are credited to the previous file.
    """
    if not line.startswith("diff --git "):
        return None
    rest = line[len("diff --git ") :].rstrip("\n")
    marker = rest.find(' "b/')
    if marker != -1:
        quoted = rest[marker + 4 :]
        if quoted.endswith('"'):
            quoted = quoted[:-1]
        return unquote_c_path(quoted) or None
    marker = rest.find(" b/")
    if marker == -1:
        return None
    return rest[marker + 3 :] or None


_HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")


def parse_diff(text: str) -> list[FileDiff]:
    """Split a unified diff into files and hunks.

    Split on ``\\n`` only: ``str.splitlines`` also breaks on form feeds and
    other separators a changed line can carry, which would turn one added line
    into a phantom context line and misnumber the rest of its hunk.
    """
    files: list[FileDiff] = []
    current: FileDiff | None = None
    hunk: Hunk | None = None
    line_no = 0
    for line in text.split("\n"):
        path = diff_header_path(line)
        if path is not None:
            current = FileDiff(path)
            files.append(current)
            hunk = None
            continue
        if hunk is None and current is not None and line.startswith("new file mode"):
            current.new_file = True
            continue
        match = _HUNK_RE.match(line)
        if match and current is not None:
            start = int(match.group(1))
            length = int(match.group(2)) if match.group(2) is not None else 1
            hunk = Hunk(start, length)
            current.hunks.append(hunk)
            line_no = start
            continue
        if hunk is None:
            continue
        if line.startswith("+"):
            hunk.added.append((line_no, line[1:]))
            line_no += 1
        elif line.startswith("-"):
            hunk.removed.append(line[1:])
        elif line.startswith("\\"):
            continue
        else:
            line_no += 1
    return files


def _span(hunk: Hunk) -> tuple[int, int]:
    """The post-image lines a hunk covers; a pure deletion covers its anchor."""
    return hunk.new_start, hunk.new_start + max(hunk.new_len, 1) - 1


def _intersects(span, ranges) -> bool:
    lo, hi = span
    return any(lo <= end and start <= hi for start, end in ranges)


def _inside(span, ranges) -> bool:
    lo, hi = span
    return any(start <= lo and hi <= end for start, end in ranges)


def is_test_hunk(path: str, kind: str, hunk: Hunk, test_ranges) -> bool:
    """A hunk in a test file, or one reaching an inline ``#[cfg(test)]`` region.

    Judged on the hunk's whole post-image span, not its start line: a NEW Rust
    file arrives as one hunk from line 1, so a start-line rule never saw its
    trailing test module.
    """
    if kind == "tests":
        return True
    return _intersects(_span(hunk), test_ranges.get(path, ()))


def test_shape(diffs: list[FileDiff], kinds: dict, test_ranges: dict) -> dict:
    """``{"hunks": n, "lines": n}`` of test material in the diff.

    What the retired tests slice told the completeness lens: an empty count on
    a Rust diff means no test moved, inline modules included.
    """
    hunks = lines = 0
    for fd in diffs:
        kind = kinds.get(fd.path, "source")
        for h in fd.hunks:
            if is_test_hunk(fd.path, kind, h, test_ranges):
                hunks += 1
                lines += len(h.added) + len(h.removed)
    return {"hunks": hunks, "lines": lines}


# --------------------------------------------------------------------------
# Shape signals for the tier.

_CONTROL_FLOW_RE = re.compile(
    r"\b(if|else|elif|match|for|while|loop|return|fn|def|class|try|except|"
    r"catch|switch|case|yield|break|continue|await|lambda)\b|=>|\?;"
)


def _code_lines(diffs, kinds):
    for fd in diffs:
        if kinds.get(fd.path, "source") == "docs":
            continue
        yield fd


def changes_control_flow(diffs, kinds) -> bool:
    """Whether any added OR removed non-docs line carries a control-flow token.

    Both sides: deleting a guard (``-if not authorized(u):``) changes control
    flow as surely as adding one, and is the riskier of the two.
    """
    return any(
        _CONTROL_FLOW_RE.search(text)
        for fd in _code_lines(diffs, kinds)
        for h in fd.hunks
        for text in [t for _, t in h.added] + h.removed
    )


# Indentation is semantics in these, so a whitespace-only change is not a
# reformat: a dedent can move a `return` out of its loop.
INDENT_SIGNIFICANT = (".py", ".yml", ".yaml")


def is_reformat(diffs, kinds) -> bool:
    """Every code file's added text equals its removed text, whitespace aside.

    A mechanical reformat: the bytes moved, the tokens did not. Raw lines must
    differ somewhere, or this is a no-op diff, not a reformat. Never true when
    an indentation-significant file changed.
    """
    total = 0
    differs = False
    for fd in _code_lines(diffs, kinds):
        added = [t for h in fd.hunks for _, t in h.added]
        removed = [t for h in fd.hunks for t in h.removed]
        if (added or removed) and fd.path.endswith(INDENT_SIGNIFICANT):
            return False
        if "".join("".join(added).split()) != "".join("".join(removed).split()):
            return False
        total += len(added) + len(removed)
        differs = differs or added != removed
    return differs and total >= SHAPE_MIN_LINES


def _substantive(text: str) -> bool:
    """A line that can carry a move: not blank, not a lone bracket or keyword.

    Filler (``}``, ``)``, ``Ok(())``, ``return``) appears on both sides of any
    rewrite, so counting it would let an in-place rewrite read as a move.
    """
    stripped = text.strip()
    return len(stripped) > 6 and any(ch.isalnum() for ch in stripped)


def moved_share(diffs, kinds) -> float:
    """Matched substantive lines over the LARGER side, removed or added.

    The larger side is the denominator so that new logic dilutes the share: a
    few re-indented deletions beside hundreds of new lines is not a move.
    """
    removed = Counter()
    added = Counter()
    for fd in _code_lines(diffs, kinds):
        for h in fd.hunks:
            removed.update(t.strip() for t in h.removed if _substantive(t))
            added.update(t.strip() for _, t in h.added if _substantive(t))
    total = max(sum(removed.values()), sum(added.values()))
    if sum(removed.values()) < SHAPE_MIN_LINES:
        return 0.0
    return sum((removed & added).values()) / total


# A char literal is ONE character or escape (`'a'`, `'\n'`, `'\u{1F600}'`): a
# wider `'…'` branch read the span between two Rust lifetimes
# (`impl<'a> Foo<'a>`) as a literal and hid the type edit inside it.
_LITERAL_RE = re.compile(
    r"\"(?:[^\"\\]|\\.)*\"|'(?:[^'\\]|\\.[^']*)'|\b\d[\d_.xXa-fA-F]*\b"
)


def _skeleton(text: str) -> str:
    """A line with its string and numeric literals replaced by ``#``."""
    return _LITERAL_RE.sub("#", text.strip())


def is_rewiring(diffs, kinds) -> bool:
    """In-place VALUE edits: each removed line pairs with an added line that
    differs only in its literals, and no control flow changes.

    The literal-skeleton check is what makes this "rewiring" rather than any
    N-for-N edit: ``a * b / c`` → ``a * b / d`` keeps its line count and still
    changes the arithmetic, so it fails here and is reviewed as code.
    """
    seen = False
    for fd in _code_lines(diffs, kinds):
        for h in fd.hunks:
            added = [t for _, t in h.added]
            if not added or len(added) != len(h.removed):
                return False
            if any(_skeleton(a) != _skeleton(r) for a, r in zip(added, h.removed)):
                return False
            seen = True
    return seen and not changes_control_flow(diffs, kinds)


# --------------------------------------------------------------------------
# Gates.


def _first(paths, predicate):
    return next((p for p in paths if predicate(p)), None)


def gates(
    files: list[dict],
    kinds: dict,
    base_trees: set[str] | None,
    prose_heavy: bool,
    added_paths: set[str] | None = None,
) -> dict:
    """Each lens gate, ``{"fires": bool, "path": <triggering path or null>}``.

    ``base_trees`` is the base's top-level directory set; ``None`` (not
    measured) leaves the new-tree gate unfired rather than guessing.
    ``added_paths`` are the files the diff ADDS: a new build manifest is a new
    subsystem the registry and the CI skip-list must learn about, even inside
    an existing top-level tree, so it fires the new-tree gate too.
    """
    paths = [f["path"] for f in files]
    security = _first(
        paths,
        lambda p: kinds.get(p) != "docs" and matches_any(p, SECURITY_SURFACE),
    )
    freshness = _first(paths, lambda p: matches_any(p, FRESHNESS_SURFACE))
    new_tree = None
    if base_trees is not None:
        new_tree = _first(
            paths, lambda p: "/" in p and p.partition("/")[0] not in base_trees
        )
    if new_tree is None and added_paths:
        new_tree = _first(
            paths, lambda p: p in added_paths and matches_any(p, BUILD_MANIFESTS)
        )
    return {
        "security": {"fires": security is not None, "path": security},
        "freshness": {"fires": freshness is not None, "path": freshness},
        "new_tree": {"fires": new_tree is not None, "path": new_tree},
        "prose_heavy": {"fires": bool(prose_heavy), "path": None},
    }


# --------------------------------------------------------------------------
# The tier.


def abridged_eligible(files: list[dict], code_crates: int) -> bool:
    """The entry gate's small-diff threshold: offer the abridged tier at all?

    ``files`` is the WHOLE inventory, generated families included: a
    dependency bump's lockfile is part of its size, and dropping it made a
    manifest-plus-lockfile change read as a two-line config tweak.
    """
    return (
        len(files) <= ABRIDGED_MAX_FILES
        and sum(f["changes"] for f in files) <= ABRIDGED_MAX_CHANGES
        and code_crates <= 1
        and not any(matches_any(f["path"], ABRIDGED_EXCLUDED) for f in files)
    )


def classify_tier(
    files: list[dict],
    kinds: dict,
    diffs: list[FileDiff],
    test_ranges: dict,
    code_crates: int,
    inventory: list[dict] | None = None,
) -> dict:
    """``{"name": <tier>, "reason": <one line>}``, the first tier that fits.

    Ordered narrowest-claim first; the full tier is the default, so a diff
    that fits nothing below it is fully reviewed. ``files`` is the reviewed
    set; ``inventory`` (default ``files``) is the whole branch, generated
    families included — the trivial tier needs both small and free of any
    generated change.
    """
    inventory = files if inventory is None else inventory
    paths = [f["path"] for f in files]
    changes = sum(f["changes"] for f in files)
    code = [p for p in paths if kinds.get(p) != "docs"]
    meta_only = all(matches_any(p, META_SURFACE) for p in paths)
    generated = len(inventory) > len(files)

    if not generated and abridged_eligible(inventory, code_crates):
        nature = None
        if not code:
            nature = "docs only"
        elif all(matches_any(p, CONFIG_SURFACE) for p in code):
            nature = "config only"
        elif not changes_control_flow(diffs, kinds):
            nature = "no control-flow change"
        if nature:
            return {
                "name": "trivial",
                "reason": f"{len(files)} file(s), {changes} changed lines, {nature}",
            }

    if code and not meta_only and all(matches_any(p, INFRA_SURFACE) for p in code):
        return {
            "name": "reduced-infra",
            "reason": "every code path is build / CI / deploy",
        }
    if is_reformat(diffs, kinds):
        return {
            "name": "reduced-reformat",
            "reason": "added text equals removed text per file, whitespace aside",
        }
    share = moved_share(diffs, kinds)
    if share >= MOVE_SHARE:
        return {
            "name": "reduced-move",
            "reason": f"{share:.0%} of removed code lines reappear as added",
        }
    if changes <= REWIRING_MAX_CHANGES and is_rewiring(diffs, kinds):
        return {
            "name": "reduced-rewiring",
            "reason": "every hunk changes only literals, no control-flow change",
        }
    production = [
        (fd.path, h)
        for fd in _code_lines(diffs, kinds)
        for h in fd.hunks
        if not (
            kinds.get(fd.path) == "tests"
            or _inside(_span(h), test_ranges.get(fd.path, ()))
        )
    ]
    # A changed production file with no hunk at all (a mode change, a binary,
    # a pure rename) is still a production change.
    with_hunks = {fd.path for fd in diffs if fd.hunks}
    no_hunk = [p for p in code if kinds.get(p) != "tests" and p not in with_hunks]
    if code and diffs and not production and not no_hunk:
        return {"name": "reduced-test-only", "reason": "every code hunk is test code"}
    tests = test_shape(diffs, kinds, test_ranges)
    if (
        code_crates == 1
        and changes <= SMALL_CRATE_MAX_CHANGES
        and tests["lines"] >= SMALL_CRATE_TEST_SHARE * changes
    ):
        return {
            "name": "small-single-crate",
            "reason": (
                f"one crate, {changes} changed lines, {tests['lines']} of them tests"
            ),
        }
    if meta_only:
        return {"name": "meta", "reason": "confined to .claude, CLAUDE.md, docs, cfg"}
    return {"name": "full", "reason": "fits no narrower tier"}


def lenses_for(tier: str, gate_set: dict) -> list[dict]:
    """The tier's lenses plus every fired gate's, cross-check last.

    Each entry is ``{"lens", "why", "required", "ask_to_skip"}``. ``why`` is
    ``tier`` or ``gate:<name>``; a lens both the tier and a gate want keeps
    ``tier``. ``ask_to_skip`` is true only for the cross-check on the trivial
    tier.
    """
    why: dict[str, str] = {name: "tier" for name in TIER_LENSES[tier]}
    for gate_name, lens_names in GATE_LENSES.items():
        if gate_set[gate_name]["fires"]:
            for name in lens_names:
                why.setdefault(name, f"gate:{gate_name}")
    why["cross-check"] = "tier"
    return [
        {
            "lens": name,
            "why": why[name],
            "required": True,
            "ask_to_skip": name == "cross-check" and tier == "trivial",
        }
        for name in LENS_ORDER
        if name in why
    ]


def subdivision(files: list[dict], diff_lines: int) -> list[dict] | None:
    """Path-scoped ``--only`` groups for an oversized diff, largest first.

    ``None`` under :data:`SUBDIVIDE_DIFF_LINES`. Grouped by top-level tree; a
    lens whose question sits inside one tree takes its group, while the
    cross-check and any cross-file lens keep the whole diff. ``files`` is the
    reviewed set, generated families already dropped.
    """
    if diff_lines <= SUBDIVIDE_DIFF_LINES:
        return None
    # One segment deep, or two when one top-level tree dominates: a
    # `.claude/**` group holding 2,929 of 2,931 changes subdivides nothing,
    # whether or not a two-line `cfg/**` group sits beside it.
    for depth in (1, 2):
        groups: Counter = Counter()
        for f in files:
            parts = f["path"].split("/")
            if len(parts) > depth:
                key = "/".join(parts[:depth]) + "/**"
            else:
                key = f["path"]
            groups[key] += f["changes"]
        total = sum(groups.values())
        top = groups.most_common(1)[0][1] if groups else 0
        if top < DOMINANT_SHARE * total:
            break
    return [{"only": glob, "changes": n} for glob, n in groups.most_common()]


# --------------------------------------------------------------------------
# The gate-integrity tagger.

_TEST_MARKER_RE = re.compile(
    r"^\s*(#\[(tokio::)?test\b|#\[test_case|def test_\w+|async def test_\w+|"
    r"(it|test|describe)\s*\(\s*['\"`])"
)

# Added-line patterns, per kind. Prose files are not scanned for these: a
# skill or convention doc names these spellings legitimately.
#
# No pattern may match its own source text, so a diff of this file tags
# nothing from these definitions. A literal needle therefore carries a
# one-character class (`[-]`, `[ ]`, `[:]`) or an escape that the source
# spells differently from the text it matches. A path allowlist is ruled
# out: it would blind the tagger in exactly the files where a real bypass
# hides best. `SelfMatchTest` holds the rule.
_ADDED_PATTERNS = (
    (
        "test-skipped",
        re.compile(
            r"#\[ignore\b|@unittest\.skip|@pytest\.mark\.(skip|xfail)|"
            r"\bself\.skipTest\(|\b(it|test|describe)\.skip\(|\bx(it|describe)\("
        ),
    ),
    (
        "placeholder",
        re.compile(
            r"\b(todo|unimplemented)!\(|raise[ ]NotImplementedError|"
            r"\bT[O]DO\b|\bF[I]XME\b|\bX[X]X\b"
        ),
    ),
    (
        "suppression",
        re.compile(
            r"#!?\[allow\(|#\s*noqa\b|#\s*type:\s*ignore|@ts-(ignore|expect-error)|"
            r"eslint[-]disable|biome[-]ignore|pragma[:] no cover|pylint:\s*disable|"
            r"cspell[:]disable|yamllint[ ]disable|#compound[-]ok|#destructive[-]ok"
        ),
    ),
    (
        "gate-bypass",
        re.compile(
            r"-[-]no-verify|\[skip ci\]|\bskip[-]ci\b|continue-on-error:\s*true|"
            r"-[-]no-gpg-sign|\|\|\s*true\b"
        ),
    ),
    (
        "swallowed-exception",
        re.compile(
            r"^\s*except(\s+[\w.,() ]+)?:\s*(pass|\.\.\.)\s*$|"
            r"catch\s*(\([^)]*\))?\s*\{\s*\}|\.ok\(\);\s*$|^\s*let _ = "
        ),
    ),
)

_EXCEPT_HEAD_RE = re.compile(r"^\s*except(\s+[\w.,() ]+)?:\s*$")

_BOUND_WORD_RE = re.compile(
    r"timeout|retr(y|ies)|attempts|tolerance|epsilon|delta|max_|min_|sleep|"
    r"deadline|backoff",
    re.IGNORECASE,
)
_NUMBER_RE = re.compile(r"\d+(\.\d+)?")
_EXACT_ASSERT_RE = re.compile(
    r"assert_eq[!]|assertEqual\(|\.toBe\(|\.toEqual\(|toHaveLength\(|assertCountEqual\("
)
_RANGE_ASSERT_RE = re.compile(
    r"assert!\(.*(>=|<=|>|<)|assert(Greater|Less)(Equal)?\(|"
    r"toBeGreater[T]han|toBeLess[T]han|assertAlmostEqual\("
)
_SNAPSHOT_PATTERNS = ("**/*.snap", "**/__snapshots__/**", "**/snapshots/**")

# No `expect(` branch: in Rust that is an unwrap, not an assertion.
_ASSERT_RE = re.compile(
    r"\b(debug_)?assert(_eq|_ne)?!\s*\(|^\s*assert\s|\bself\.assert\w*\("
)

INTEGRITY_KINDS = (
    "test-removed",
    "assertion-removed",
    "test-skipped",
    "snapshot-without-test",
    "placeholder",
    "suppression",
    "gate-bypass",
    "relaxed-bound",
    "relaxed-assertion",
    "swallowed-exception",
)

# Hits printed in the verdict; `count` still reports every one.
INTEGRITY_SHOWN = 20


def _hit(kind, path, line, text):
    return {"kind": kind, "path": path, "line": line, "text": text.strip()[:120]}


def integrity_hits(
    diffs: list[FileDiff], kinds: dict, test_ranges: dict | None = None
) -> list[dict]:
    """Every hunk-level gate-integrity hit, in diff order.

    ``test_ranges`` (inline ``#[cfg(test)]`` regions) lets an assertion
    removed from an inline Rust test count as a test change.

    Deterministic and deliberately over-inclusive: a hit is a question for the
    operator (restore, keep as justified, keep for another reason, leave open),
    never a finding, and never auto-fixed.
    """
    hits: list[dict] = []
    changed_tests = any(
        kinds.get(fd.path) == "tests" and fd.hunks for fd in diffs
    ) or any(
        _TEST_MARKER_RE.search(t) for fd in diffs for h in fd.hunks for _, t in h.added
    )
    for fd in diffs:
        kind = kinds.get(fd.path, "source")
        if matches_any(fd.path, _SNAPSHOT_PATTERNS) and not changed_tests:
            hits.append(_hit("snapshot-without-test", fd.path, None, fd.path))
        removed_tests = [
            t for h in fd.hunks for t in h.removed if _TEST_MARKER_RE.search(t)
        ]
        added_tests = sum(
            1 for h in fd.hunks for _, t in h.added if _TEST_MARKER_RE.search(t)
        )
        if len(removed_tests) > added_tests:
            for text in removed_tests[added_tests:]:
                hits.append(_hit("test-removed", fd.path, None, text))
        # Deleting an assertion from a test that stays is the commonest way to
        # weaken one, and no test marker moves when it happens.
        test_hunks = [
            h for h in fd.hunks if is_test_hunk(fd.path, kind, h, test_ranges or {})
        ]
        for text in _dropped_asserts(test_hunks):
            hits.append(_hit("assertion-removed", fd.path, None, text))
        if kind == "docs":
            continue
        for h in fd.hunks:
            for i, (line_no, text) in enumerate(h.added):
                for name, pattern in _ADDED_PATTERNS:
                    if pattern.search(text):
                        hits.append(_hit(name, fd.path, line_no, text))
                # A bare `except …:` whose next added line only passes.
                if (
                    _EXCEPT_HEAD_RE.match(text)
                    and i + 1 < len(h.added)
                    and h.added[i + 1][1].strip() in ("pass", "...")
                ):
                    hits.append(_hit("swallowed-exception", fd.path, line_no, text))
            hits.extend(_relaxations(fd.path, h))
    return hits


def _dropped_asserts(hunks: list[Hunk]) -> list[str]:
    """The removed assertions the added ones do not replace, in diff order.

    A removed assertion pairs first with an added one of the same text (a
    moved line), then with one of the same literal skeleton (an edited
    value), so the quoted text is the one actually lost rather than
    whichever came last. The count still nets removed against added: an
    unpaired added assertion offsets one unpaired removed assertion, and the
    earliest unpaired ones are taken as rewritten.
    """
    removed = [t.strip() for h in hunks for t in h.removed if _ASSERT_RE.search(t)]
    added = [t.strip() for h in hunks for _, t in h.added if _ASSERT_RE.search(t)]
    for key in (str, _skeleton):
        pool = Counter(key(t) for t in added)
        kept_removed, matched = [], Counter()
        for text in removed:
            if pool[key(text)]:
                pool[key(text)] -= 1
                matched[key(text)] += 1
            else:
                kept_removed.append(text)
        kept_added = []
        for text in added:
            if matched[key(text)]:
                matched[key(text)] -= 1
            else:
                kept_added.append(text)
        removed, added = kept_removed, kept_added
    return removed[len(added) :]


def _relaxations(path: str, hunk: Hunk) -> list[dict]:
    """A bound whose number changed, or an exact assertion turned into a range."""
    out = []
    removed_skeletons = {
        _NUMBER_RE.sub("#", t.strip()): t for t in hunk.removed if _NUMBER_RE.search(t)
    }
    exact_removed = any(_EXACT_ASSERT_RE.search(t) for t in hunk.removed)
    for line_no, text in hunk.added:
        skeleton = _NUMBER_RE.sub("#", text.strip())
        old = removed_skeletons.get(skeleton)
        if (
            old is not None
            and old.strip() != text.strip()
            and _BOUND_WORD_RE.search(text)
        ):
            out.append(_hit("relaxed-bound", path, line_no, text))
        elif exact_removed and _RANGE_ASSERT_RE.search(text):
            out.append(_hit("relaxed-assertion", path, line_no, text))
    return out


def integrity_summary(hits: list[dict]) -> dict:
    """``{"count", "by_kind", "hits"}``, the hits capped at :data:`INTEGRITY_SHOWN`."""
    by_kind = Counter(h["kind"] for h in hits)
    return {
        "count": len(hits),
        "by_kind": {k: by_kind[k] for k in INTEGRITY_KINDS if by_kind[k]},
        "hits": hits[:INTEGRITY_SHOWN],
    }
