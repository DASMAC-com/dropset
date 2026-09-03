#!/usr/bin/env python3
"""Verify a merge resolution is COMPLETE against both parents, not merely
well-formed — every line either parent added survives, per side, or is
explicitly acknowledged as superseded.

**Why a linter cannot do this.** The lint hooks over the shared ordered files
verify **sortedness**, and sortedness is preserved by dropping a line. So a
resolution that silently loses one side's contribution compiles, lints, sorts,
and reviews clean. Nothing else in the repo checks completeness.

**The failure this exists to catch.** In one resolution a doc comment reading
"all four kinds" merged *cleanly* — both parents had independently written
"four", each counting a different new variant, and the true post-merge answer
was five. Line-based completeness flagged it because neither parent's line
survived verbatim. An identifier-set check would have missed it: identifier
sets are neat for a fence list and blind to prose.

**The value is forced adjudication, not a green light.** That bug was caught by
this check *failing* and making eight apparent losses be explained one at a
time. A pass is much weaker than it looks — in the very same resolution both
branches added a variant to one enum and an arm to one match, every line from
both sides survived, completeness passed trivially, and the result was still
wrong in a way only reading revealed. So never read a pass as authorization to
skip reading the merge.

**Which files this is for.** Any shared file whose content is an ordered or
keyed list that several branches append to:

The **alphabetically-keyed YAML** under ``cfg/`` and ``infra/aws/`` is the
standing case: those files have no structural escape from the shared insertion
point, so a conflict in them is resolved by hand.

``cfg/dictionary.txt`` is deliberately **not** on that list. It carries a
``merge=union`` attribute precisely so it is never hand-resolved (``CLAUDE.md``
→ "Docs and skills prose" states that as a rule), and this tool is for files
you *do* resolve by hand. Its one residual hazard — union merge resurrecting a
word one side deleted — is also invisible here, since a resurrected line is a
*base* line reappearing rather than one either parent added; the next
spelling-hygiene pass is what catches that.

**Two bounds, stated because the obvious guess is wrong both times.**

It checks that every line each parent *added* survives, so a **deletion** that
one side made and the resolution undid is never examined. Additions only.

And comparison is **set-based over normalized lines**, so "ours added this line
twice, the resolution kept one" reports as survived. That bound bites the YAML
case above more than it first appears: duplicate *keys* are forbidden, but
duplicate *lines* are routine in nested YAML — ``  Enabled: true`` and
``    - Effect: Allow`` repeat freely down a CloudFormation template. So on
deeply-nested YAML this under-reports a lost repeat, and on prose it under-
reports outright. Read a COMPLETE verdict as "no *distinct* added line went
missing", which is weaker than it sounds and is still the thing a sortedness
check cannot tell you at all.

The central schema-fence relation list — the file that originally motivated the
tool — was retired when each migration moved to its own fence sidecar, so that
conflict class is gone by construction. The general lever outlived its first
target.

Usage::

    # During a conflicted merge or rebase, against the working-tree resolution
    python3 .claude/tools/merge_completeness.py --path cfg/dictionary.txt

    # Explicit revisions
    python3 .claude/tools/merge_completeness.py --path cfg/dictionary.txt \\
        --base $(git merge-base HEAD MERGE_HEAD) --ours HEAD --theirs MERGE_HEAD

    # Acknowledge a deliberate supersession, repeatably
    python3 .claude/tools/merge_completeness.py --path cfg/x.yml \\
        --acknowledge 'old_key: value'

Exit status is 0 only when every added line from both sides survives or is
acknowledged. Standard library only; shells out to ``git show`` for the three
blobs. A Python skill-tool under ``.claude/tools/`` — deliberately **not** a
Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling"). Tests live in
``tests/test_merge_completeness.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

# The default revisions during a conflicted merge. `MERGE_HEAD` exists only
# mid-merge; during a rebase the incoming side is `REBASE_HEAD`, which is why
# both are tried rather than assuming a merge.
_THEIRS_CANDIDATES = ("MERGE_HEAD", "REBASE_HEAD")


class MergeCompletenessError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def normalize(line: str) -> str:
    """A line reduced to what a completeness comparison should care about.

    Leading/trailing whitespace goes and internal runs collapse, because
    re-indentation is not a loss — and treating it as one produced two measured
    FALSE losses, which is the failure that makes a checker get ignored. An
    empty line normalizes to empty and is dropped by the callers below: blank
    lines carry no contribution and their counts are pure noise.
    """
    return " ".join(line.split())


def normalized_set(text: str) -> dict[str, str]:
    """Normalized line → its first raw spelling, in first-appearance order.

    A dict rather than a set so a reported miss can be shown in the spelling the
    author actually wrote, while comparison stays normalization-insensitive.
    """
    out: dict[str, str] = {}
    for raw in text.splitlines():
        key = normalize(raw)
        if key and key not in out:
            out[key] = raw
    return out


def git_show(rev: str, path: str, repo: Path) -> str:
    """The blob at ``rev:path``. A missing path is empty, not an error.

    A file added on only one side is a legitimate shape — it simply has no base
    content — so `git show` failing for that reason must not abort the check.
    """
    try:
        completed = subprocess.run(
            ["git", "-C", str(repo), "show", f"{rev}:{path}"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as e:
        raise MergeCompletenessError(f"cannot run git: {e}") from e
    if completed.returncode != 0:
        return ""
    return completed.stdout


def resolve_theirs(repo: Path) -> str:
    """Whichever of MERGE_HEAD / REBASE_HEAD this repo currently has."""
    for candidate in _THEIRS_CANDIDATES:
        completed = subprocess.run(
            ["git", "-C", str(repo), "rev-parse", "--verify", "--quiet", candidate],
            capture_output=True,
            text=True,
            check=False,
        )
        if completed.returncode == 0:
            return candidate
    raise MergeCompletenessError(
        "no merge in progress (neither MERGE_HEAD nor REBASE_HEAD exists) — "
        "pass --base/--ours/--theirs explicitly"
    )


def merge_base(ours: str, theirs: str, repo: Path) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repo), "merge-base", ours, theirs],
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        raise MergeCompletenessError(f"no merge base between {ours} and {theirs}")
    return completed.stdout.strip()


def audit(
    base: str,
    ours: str,
    theirs: str,
    resolution: str,
    acknowledged: tuple[str, ...] = (),
) -> dict:
    """Per-side survival of every line each parent added. Pure; no git, no IO.

    Returns counts plus the itemized misses, so the caller can report every
    apparent loss individually rather than a single pass/fail verdict.
    """
    base_lines = normalized_set(base)
    ours_added = {k: v for k, v in normalized_set(ours).items() if k not in base_lines}
    theirs_added = {
        k: v for k, v in normalized_set(theirs).items() if k not in base_lines
    }
    resolved = normalized_set(resolution)
    ack = {normalize(a) for a in acknowledged if normalize(a)}

    def split(added: dict[str, str]) -> tuple[list[str], list[str], list[str]]:
        survived, superseded, missing = [], [], []
        for key, raw in added.items():
            if key in resolved:
                survived.append(raw)
            elif key in ack:
                superseded.append(raw)
            else:
                missing.append(raw)
        return survived, superseded, missing

    ours_survived, ours_superseded, ours_missing = split(ours_added)
    theirs_survived, theirs_superseded, theirs_missing = split(theirs_added)

    union = set(ours_added) | set(theirs_added)
    return {
        "ours": {
            "added": len(ours_added),
            "survived": len(ours_survived),
            "acknowledged": ours_superseded,
            "missing": ours_missing,
        },
        "theirs": {
            "added": len(theirs_added),
            "survived": len(theirs_survived),
            "acknowledged": theirs_superseded,
            "missing": theirs_missing,
        },
        "union_added": len(union),
        "resolution_lines": len(resolved),
        "complete": not (ours_missing or theirs_missing),
        # An acknowledgement that matched nothing is worth surfacing: it usually
        # means the line was misquoted, and a misquoted acknowledgement silently
        # fails to cover the loss it was written for.
        "unused_acknowledgements": sorted(
            a for a in ack if a not in ours_added and a not in theirs_added
        ),
    }


def report(verdict: dict, ours_label: str, theirs_label: str) -> list[str]:
    """The human-facing lines. Per-side counts first, then every miss itemized."""
    out = [
        f"{ours_label} contributed {verdict['ours']['added']}, "
        f"{theirs_label} contributed {verdict['theirs']['added']}, "
        f"union {verdict['union_added']}, "
        f"resolution {verdict['resolution_lines']}"
    ]
    for label, side in ((ours_label, "ours"), (theirs_label, "theirs")):
        data = verdict[side]
        if data["acknowledged"]:
            out.append(
                f"  {label}: {len(data['acknowledged'])} acknowledged as superseded"
            )
            for raw in data["acknowledged"]:
                out.append(f"    ~ {raw}")
        if data["missing"]:
            out.append(f"  {label}: {len(data['missing'])} MISSING from the resolution")
            for raw in data["missing"]:
                out.append(f"    - {raw}")
    for unused in verdict["unused_acknowledgements"]:
        out.append(f"  WARNING: acknowledgement matched no added line: {unused}")
    out.append("COMPLETE" if verdict["complete"] else "INCOMPLETE")
    return out


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="merge_completeness.py",
        description=("Verify a merge resolution keeps every line both parents added."),
    )
    parser.add_argument("--path", required=True, help="repo-relative file path")
    parser.add_argument("--repo", default=".", help="repo root (default cwd)")
    parser.add_argument("--base", default=None, help="merge base (default: computed)")
    parser.add_argument("--ours", default="HEAD", help="our side (default HEAD)")
    parser.add_argument(
        "--theirs",
        default=None,
        help="their side (default: MERGE_HEAD or REBASE_HEAD)",
    )
    parser.add_argument(
        "--resolution",
        default=None,
        help="the resolved file (default: the working-tree copy of --path)",
    )
    parser.add_argument(
        "--acknowledge",
        action="append",
        default=None,
        metavar="LINE",
        help="a line deliberately superseded rather than lost; repeatable. "
        "Compared whitespace-insensitively, like everything else",
    )
    args = parser.parse_args(argv[1:])

    repo = Path(args.repo)
    theirs = args.theirs or resolve_theirs(repo)
    base = args.base or merge_base(args.ours, theirs, repo)

    resolution_path = Path(args.resolution) if args.resolution else repo / args.path
    try:
        resolution = resolution_path.read_text(encoding="utf-8")
    except OSError as e:
        raise MergeCompletenessError(f"cannot read {resolution_path}: {e}") from e

    # An UNRESOLVED file trivially contains every line from both parents, so it
    # would score COMPLETE and exit 0 — the strongest possible green at the
    # exact moment nothing has been resolved. Refuse instead. This matters
    # because the primary documented invocation runs against the working-tree
    # copy mid-conflict, which is precisely when markers are present.
    # `=======` is matched EXACTLY, not as a prefix. A conflict divider is that
    # line and nothing else, whereas a prefix test also refuses any file
    # carrying a setext-style heading underline or an `=======` rule — a false
    # refusal on ordinary content, and this tool advertises itself for any
    # ordered or keyed file.
    def is_marker(line):
        return (
            line.startswith("<<<<<<< ")
            or line.startswith(">>>>>>> ")
            or line.strip() == "======="
        )

    if any(is_marker(line) for line in resolution.splitlines()):
        raise MergeCompletenessError(
            f"{resolution_path} still contains conflict markers — resolve it "
            f"first; an unresolved file contains both sides by construction "
            f"and would report COMPLETE"
        )

    base_text = git_show(base, args.path, repo)
    ours_text = git_show(args.ours, args.path, repo)
    theirs_text = git_show(theirs, args.path, repo)

    # `git_show` returns "" for any non-zero git exit, which is right for a file
    # that legitimately does not exist on one side — but all three empty means
    # the revisions or the path are wrong, and the audit would then be
    # vacuously complete. A typo must not read as a pass.
    if not (base_text or ours_text or theirs_text):
        raise MergeCompletenessError(
            f"none of {base}, {args.ours}, {theirs} contains '{args.path}' — "
            f"check the revisions and the path; an all-empty comparison would "
            f"report COMPLETE having examined nothing"
        )

    verdict = audit(
        base_text,
        ours_text,
        theirs_text,
        resolution,
        tuple(args.acknowledge or ()),
    )

    for line in report(verdict, args.ours, theirs):
        print(line)
    return 0 if verdict["complete"] else 1


def main() -> int:
    try:
        return run(sys.argv)
    except MergeCompletenessError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
