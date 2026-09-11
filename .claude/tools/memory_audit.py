#!/usr/bin/env python3
"""Staleness candidates for ``housekeeping`` step 8's auto-memory review.

The step-8 *cadence gate* is already a tool (``memory_scan_gate.py``); the scan
that gate authorizes was the last step in the pass still prescribed as prose, so
every pass improvised the same three shapes against the memory store. Measured on
one pass (session ``eea25c27``): the improvised reads were ≈3.2k of ≈8.3k total
Bash bytes — ~39%, and four of the pass's top six single results. An ``ls`` of the
store printed all 97 filenames to answer what the already-in-context
``MEMORY.md`` index answers, and an ``awk`` one-liner returned 56 rows of
over-long index lines when the decision needed only the count.

So this prints **slug + one-line reason** candidates and a summary, and **never a
memory body** — the report is what enters the main loop, and a body would defeat
the point. `housekeeping` step 8 drives it between the gate's ``check`` and
``record``.

WHAT IT CHECKS, and each one's confidence, because they are not equal:

* **index desync** (exact) — a ``MEMORY.md`` pointer whose target file is
  missing, or a memory file with no pointer. The step's own purge rule is "delete
  the file AND remove its pointer", so a half-done purge is precisely this.
* **over-long index lines** (exact) — reported as a **count plus the worst few**,
  never all of them.
* **dangling repo paths** (exact, bounded) — a code-span repo-relative path in a
  memory body that does not resolve under the repo root.
* **superseded candidates** (heuristic) — several memories sharing a slug stem.
  Reported as a candidate to look at, **never a verdict**; the newest is not
  automatically right and the tool cannot read intent.

WHAT IT DELIBERATELY DOES NOT CHECK. An ``ENG-###`` reference is **not** verified,
even though a dangling one is exactly the staleness this step hunts. Issue
existence lives in Linear, this is a stdlib-only offline tool, and a check that
cannot run is worse than an absent one because its silence reads as a pass. Verify
an ``ENG-###`` through the Linear MCP if a candidate's reason turns on one.

The tool decides nothing and deletes nothing: `housekeeping` keeps the autonomy
bound (attended = confirm via ``AskUserQuestion`` before deleting; one-shot =
list only), because losing a still-good memory is worse than keeping a stale one.

Stdlib only; a Python skill-tool under ``.claude/tools/`` — deliberately **not** a
Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

# cspell:word strerror

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

#: `MEMORY.md` is an index — one line per memory. Past this width a line has
#: started carrying content that belongs in the topic file it points at, which
#: costs every session that loads the index. Matches the awk threshold the
#: improvised shape used, so the tool reports the same set it replaces.
DEFAULT_MAX_INDEX_LINE = 160

#: How many over-long lines to name. The count is the decision input; the worst
#: few are what make it actionable. All 56 were the measured waste.
DEFAULT_WORST = 3

#: A `MEMORY.md` pointer: `- [Title](file.md) — hook`.
#:
#: The title is matched **non-greedily with brackets allowed inside it**, and the
#: target must end in `.md`. A tighter `\[[^\]]*\]` looks right and is wrong: a
#: real index line reads
#: `- [anchor v2 #[program] cfg limitation](anchor-v2-program-cfg-limitation.md)`,
#: where the title itself contains `]`, so the character class stopped inside the
#: title and the line parsed as no pointer at all. That produced a false
#: `index-desync` finding against the live store for a memory that is indexed
#: perfectly well — the worst kind of false positive, since this is one of the two
#: checks reported as exact.
#: Two residuals closed after the bracket fix, both the same family — the title
#: class is permissive, so it can wander past the link it should have matched:
#: a title may not contain `](`, which stops `.*?` skipping a non-`.md` first
#: link and latching onto a later one (`- [T](x.png) — see [y](z.md)` used to
#: yield `z.md`); and the target tolerates a trailing `#anchor`, which otherwise
#: failed `\.md\)` outright and reproduced the same false `index-desync` the
#: bracket bug caused.
POINTER_RE = re.compile(r"^\s*[-*]\s*\[(?:(?!\]\().)*\]\(([^)#]*\.md)(?:#[^)]*)?\)")

#: A code-span token. Path candidates are drawn only from inside a code span —
#: prose naming a file without them is too loose to check without false alarms,
#: and a false dangling-path report costs a human a verification round trip.
BACKTICK_RE = re.compile(r"`([^`\n]+)`")

#: Path-shaped: at least one slash, and no character that means it is something
#: else (whitespace, a shell special character, a URL scheme's colon).
PATH_SHAPE_RE = re.compile(r"\A[A-Za-z0-9._*?/{}\[\]-]+\Z")

#: Trailing punctuation a path picks up from prose inside its backticks.
TRAILING_PUNCTUATION = ".,;:"


class MemoryAuditError(Exception):
    """A user-facing error (bad args, missing store)."""


def _slug(path: Path) -> str:
    return path.stem


def index_pointers(index_text: str) -> list[str]:
    """Every pointer target named in ``MEMORY.md``, in order."""
    targets = []
    for line in index_text.splitlines():
        m = POINTER_RE.match(line)
        if m:
            targets.append(m.group(1).strip())
    return targets


def over_long_index_lines(index_text: str, max_width: int) -> list[tuple[int, int]]:
    """``(line_number, width)`` for each index line past ``max_width``, widest
    first. Blank and heading lines are skipped — only pointer-ish content counts,
    since a long prose paragraph in the index is a different problem."""
    rows = []
    for i, line in enumerate(index_text.splitlines(), start=1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if len(line) > max_width:
            rows.append((i, len(line)))
    rows.sort(key=lambda r: r[1], reverse=True)
    return rows


def path_shaped_spans(body: str) -> list[str]:
    """Every code-span token that LOOKS like a repo-relative path, pre-anchor.

    Separate from :func:`path_candidates` so the caller can tell "this store cites
    no paths" from "every path it cites was dropped by the anchor". Those are
    indistinguishable in the candidate list and mean opposite things: the first is
    a clean store, the second is a check that did not run.
    """
    out: list[str] = []
    for raw in BACKTICK_RE.findall(body):
        token = raw.strip().rstrip(TRAILING_PUNCTUATION)
        if not token or "/" not in token:
            continue
        if "://" in token or token.startswith(("~", "/", "op:")):
            continue
        if not PATH_SHAPE_RE.match(token):
            continue
        out.append(token.rstrip("/"))
    return out


def path_candidates(body: str, top_level: frozenset[str]) -> list[str]:
    """Repo-relative path-shaped tokens from a memory body's code spans.

    ``top_level`` is the set of names directly under the repo root, and a token
    qualifies only when its **first segment is one of them**. That anchor is what
    makes the check trustworthy, and it is not a nicety: "contains a slash" was
    the first cut, and against the real store it reported 25 candidates of which
    about 4 were real. The other 21 were all things that are slash-shaped and are
    not paths — currency pairs (``EUR/USDT``), a CIDR block (``10.0.0.0/16``),
    pinned GitHub Action refs (``<owner>/<action>``), git refs (``origin/main``,
    ``refs/pull/N/merge``), repo slugs (``DASMAC-com/dropset``), a scheme-less URL
    (``api.kraken.com/0/public``) and a secret reference
    (``dropset/oanda/api-key``). None of those first segments is a repo entry, so
    the anchor removes every one of them.

    A scanner whose candidates are mostly false is worse than no scanner here,
    because the step's autonomy bound makes a human adjudicate each one.

    The bound this accepts in exchange: a cited path whose **own top-level
    directory** has been deleted is no longer seen. That is the rarer event and a
    far louder one — and the alternative was a report nobody could trust.
    """
    return [
        token
        for token in path_shaped_spans(body)
        if token.split("/", 1)[0] in top_level
    ]


def repo_top_level(repo_root: Path) -> frozenset[str]:
    """Names directly under the repo root — the anchor set for path candidates."""
    try:
        return frozenset(p.name for p in repo_root.iterdir())
    except OSError:
        # An unreadable root disables the path check rather than failing the scan;
        # the other three checks do not depend on the repo at all.
        return frozenset()


def resolves(repo_root: Path, token: str) -> bool:
    """Whether ``token`` names something under ``repo_root``.

    Globs are resolved with ``glob`` rather than rejected, since agent material
    routinely cites a path family (``.claude/tools/*.py``), and a family that
    matches nothing is just as stale as a missing file.
    """
    if any(ch in token for ch in "*?[{"):
        try:
            return any(repo_root.glob(token))
        except (ValueError, OSError):
            # An unparseable pattern is not evidence of staleness.
            return True
    return (repo_root / token).exists()


def slug_stem(slug: str) -> str | None:
    """The grouping key for the superseded heuristic, or None when there is none.

    ``eng-1194-foo`` and ``eng-1194-bar`` share ``eng-1194``; two unrelated
    memories share nothing. Only an ``eng-<digits>`` prefix is used as a stem —
    a generic first word (``feedback``, ``ci``) groups memories that merely share
    a topic, which produced noise rather than candidates.
    """
    m = re.match(r"\A(eng-\d+)-", slug)
    return m.group(1) if m else None


def audit(
    memory_dir: Path,
    repo_root: Path,
    max_index_line: int = DEFAULT_MAX_INDEX_LINE,
) -> dict:
    """The whole scan as data. Pure apart from reading the two inputs."""
    if not memory_dir.is_dir():
        raise MemoryAuditError(f"no memory store at {memory_dir}")

    files = sorted(p for p in memory_dir.glob("*.md") if p.is_file())
    memories = [p for p in files if p.name != "MEMORY.md"]
    index_path = memory_dir / "MEMORY.md"
    index_text = index_path.read_text(encoding="utf-8") if index_path.is_file() else ""

    pointers = index_pointers(index_text)
    pointed_at = {Path(t).name for t in pointers}
    present = {p.name for p in memories}
    top_level = repo_top_level(repo_root)

    findings: list[dict] = []

    # --- index desync (exact) ------------------------------------------------
    for target in pointers:
        if Path(target).name not in present:
            findings.append(
                {
                    "kind": "index-desync",
                    "slug": Path(target).stem,
                    "reason": f"MEMORY.md points at {target}, which is not in the store",
                }
            )
    for path in memories:
        if path.name not in pointed_at:
            findings.append(
                {
                    "kind": "index-desync",
                    "slug": _slug(path),
                    "reason": "memory file has no MEMORY.md pointer",
                }
            )

    # --- dangling repo paths (exact, bounded) --------------------------------
    # Counted so the report can distinguish "cites no paths" from "every cited
    # path was dropped by the anchor" — the second is a check that did not run,
    # and the two look identical in the findings list.
    path_shaped_total = 0
    anchored_total = 0
    for path in memories:
        try:
            body = path.read_text(encoding="utf-8")
        except OSError as exc:
            findings.append(
                {
                    "kind": "unreadable",
                    "slug": _slug(path),
                    # `exc.strerror`, never `str(exc)`. The latter is
                    # `[Errno N] msg: '<full path>'`, and this store lives under
                    # the operator's home — so it would put an absolute
                    # `/Users/<name>/…` path into a report that `housekeeping`
                    # files, one copy-paste from a Linear body. The slug already
                    # names which memory it was.
                    "reason": f"cannot read the memory file: "
                    f"{exc.strerror or 'unreadable'}",
                }
            )
            continue
        shaped = path_shaped_spans(body)
        candidates = [t for t in shaped if t.split("/", 1)[0] in top_level]
        path_shaped_total += len(shaped)
        anchored_total += len(candidates)
        missing = sorted({t for t in candidates if not resolves(repo_root, t)})
        if missing:
            shown = ", ".join(missing[:3])
            if len(missing) > 3:
                shown += f", +{len(missing) - 3} more"
            findings.append(
                {
                    "kind": "dangling-path",
                    "slug": _slug(path),
                    "reason": f"names {len(missing)} path(s) absent at HEAD: {shown}",
                }
            )

    # --- superseded candidates (heuristic) ----------------------------------
    stems: dict[str, list[str]] = {}
    for path in memories:
        stem = slug_stem(_slug(path))
        if stem:
            stems.setdefault(stem, []).append(_slug(path))
    for stem, slugs in sorted(stems.items()):
        if len(slugs) > 1:
            findings.append(
                {
                    "kind": "superseded-candidate",
                    "slug": ", ".join(sorted(slugs)),
                    "reason": (
                        f"{len(slugs)} memories share the stem {stem} — "
                        "check whether one supersedes the other (heuristic)"
                    ),
                }
            )

    # --- over-long index lines (exact, count-first) -------------------------
    long_lines = over_long_index_lines(index_text, max_index_line)

    return {
        "memory_dir": str(memory_dir),
        "memories": len(memories),
        "findings": findings,
        "over_long_index_lines": len(long_lines),
        "over_long_worst": long_lines,
        "max_index_line": max_index_line,
        # Carried so `render` can say the path check DID NOT RUN. With an empty
        # anchor set every candidate is dropped, so the check reports zero
        # findings — indistinguishable from a pass. `--repo-root` defaults to the
        # cwd, so being invoked from the wrong directory produces exactly that,
        # with no OSError involved. This tool's own docstring states the
        # principle: a check that cannot run is worse than an absent one, because
        # its silence reads as a pass.
        "repo_top_level": len(top_level),
        "paths_shaped": path_shaped_total,
        "paths_anchored": anchored_total,
    }


def render(result: dict, worst: int) -> list[str]:
    """The report: one line per finding, then a summary. No bodies, ever.

    Every list here is **bounded by ``worst``**, including the findings. The
    over-long check was capped from the start and the findings list was not, which
    left the tool's own worst failure mode unbounded: a missing or reformatted
    ``MEMORY.md`` — or any regression in ``POINTER_RE``, the bug already fixed once
    here — makes *every* memory report "no pointer", which is ~96 lines into the
    main loop. That is more than the improvised shapes this tool replaced.
    """
    lines = []

    # Say it when the path check did not actually run. Zero dangling-path findings
    # from a disabled check reads exactly like a clean store, and the wrong-root
    # case does NOT raise: a readable directory that simply is not this repo has a
    # perfectly good top-level listing, and every cited path is then dropped for
    # having an unknown first segment. So the signal is the DROP RATE, not the
    # root — an unreadable root is only the degenerate case of it.
    shaped = result.get("paths_shaped", 0)
    if not result.get("repo_top_level"):
        lines.append(
            "path-check-DISABLED: the repo root could not be listed, so no cited "
            "path was checked — pass --repo-root, or run from the repo root"
        )
    elif shaped and not result.get("paths_anchored", 0):
        lines.append(
            f"path-check-DISABLED: all {shaped} cited path(s) were dropped as "
            f"unrecognized, which means --repo-root is almost certainly not this "
            f"repo — no dangling path was checked"
        )

    # Grouped by kind so the cap applies per kind: capping the flat list would let
    # a flood of one kind hide a single finding of another.
    by_kind: dict[str, list[dict]] = {}
    for f in result["findings"]:
        by_kind.setdefault(f["kind"], []).append(f)
    for kind in sorted(by_kind):
        group = by_kind[kind]
        for f in group[:worst]:
            lines.append(f"{f['kind']}: {f['slug']} — {f['reason']}")
        if len(group) > worst:
            # Deliberately NOT prefixed `{kind}:` — an overflow marker sharing the
            # finding prefix is indistinguishable from a finding to anything
            # counting lines, including a test.
            lines.append(
                f"-- +{len(group) - worst} more {kind} not shown "
                f"(raise --worst to see them)"
            )

    count = result["over_long_index_lines"]
    if count:
        # Count first, then the worst few. Naming all of them was the measured
        # waste — 56 rows for a decision that needed one number.
        shown = result["over_long_worst"][:worst]
        where = ", ".join(f"line {n} ({w} chars)" for n, w in shown)
        lines.append(
            f"over-long-index: {count} MEMORY.md line(s) past "
            f"{result['max_index_line']} chars; worst: {where}"
        )

    kinds: dict[str, int] = {}
    for f in result["findings"]:
        kinds[f["kind"]] = kinds.get(f["kind"], 0) + 1
    breakdown = ", ".join(f"{k}={v}" for k, v in sorted(kinds.items())) or "none"
    lines.append(
        f"memory-audit | {result['memories']} memories | "
        f"{len(result['findings'])} finding(s) ({breakdown}) | "
        f"{count} over-long index line(s)"
    )
    return lines


HELP = """\
Usage:
  memory_audit.py MEMORY_DIR [--repo-root PATH] [--worst N]
                             [--max-index-line N] [--json]

Print staleness CANDIDATES for housekeeping step 8 — slug + one-line reason,
never a memory body. Exit 0 whether or not anything was found: candidates are
input to a human decision, not a failure.

  --repo-root PATH    resolve cited paths against this root (default: cwd)
  --worst N           how many over-long index lines to name (default 3)
  --max-index-line N  index-line width that counts as over-long (default 160)
  --json              emit the raw finding data instead of the report"""


def _parse_args(args: list[str]) -> tuple[Path, Path, int, int, bool]:
    memory_dir: Path | None = None
    repo_root = Path.cwd()
    worst = DEFAULT_WORST
    max_index_line = DEFAULT_MAX_INDEX_LINE
    as_json = False

    def _int(flag: str, value: str) -> int:
        try:
            return int(value)
        except ValueError as e:
            raise MemoryAuditError(f"{flag}: not an integer: {value}") from e

    i = 0
    while i < len(args):
        arg = args[i]
        if arg == "--repo-root":
            i += 1
            if i >= len(args):
                raise MemoryAuditError("--repo-root requires a value")
            repo_root = Path(args[i])
        elif arg == "--worst":
            i += 1
            if i >= len(args):
                raise MemoryAuditError("--worst requires a value")
            worst = _int("--worst", args[i])
        elif arg == "--max-index-line":
            i += 1
            if i >= len(args):
                raise MemoryAuditError("--max-index-line requires a value")
            max_index_line = _int("--max-index-line", args[i])
        elif arg == "--json":
            as_json = True
        elif arg.startswith("-"):
            raise MemoryAuditError(f"unknown argument: {arg} (try --help)")
        elif memory_dir is None:
            memory_dir = Path(arg)
        else:
            raise MemoryAuditError(f"unexpected extra argument: {arg}")
        i += 1

    if memory_dir is None:
        raise MemoryAuditError("a MEMORY_DIR argument is required")
    return memory_dir, repo_root, worst, max_index_line, as_json


def run(argv: list[str]) -> int:
    args = argv[1:]
    if not args or any(a in ("-h", "--help") for a in args):
        print(HELP)
        return 0
    memory_dir, repo_root, worst, max_index_line, as_json = _parse_args(args)
    result = audit(memory_dir, repo_root, max_index_line)
    if as_json:
        # `--worst` binds here too. Dumping `result` whole emitted EVERY over-long
        # row — measured at 51 against the live store — which is the same
        # unbounded shape this tool was built to replace, and it silently
        # contradicted the docstring's "never all of them". The full count stays
        # in `over_long_index_lines`, so nothing is lost but the volume.
        bounded = dict(result)
        bounded["over_long_worst"] = result["over_long_worst"][:worst]
        print(json.dumps(bounded, indent=2))
    else:
        for line in render(result, worst):
            print(line)
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except MemoryAuditError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
