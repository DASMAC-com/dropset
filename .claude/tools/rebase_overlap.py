#!/usr/bin/env python3
"""Triage what a rebase actually pulled in, and whether it can stale anything.

When ``main`` moves under a long-running review, ``review-pr`` rebases and then
has to decide whether its expensive gates — the regeneration checks and the full
local test suite — need re-running against the new base. That decision is pure
path arithmetic, and it was hand-rolled per rebase: fetch the base, ``log`` the
new commits, ``diff --name-only`` in both directions, then intersect the two
file sets by eye. One session ran that identical sequence **three times** as
``main`` moved 15 commits, and re-ran the whole suite (lint + IDL + SDK clients
+ conformance vectors + both Rust targets + SDK node tests) each time — runs 2
and 3 provably redundant, because each rebase delta was TS-only and touched no
input to any generated artifact. The session reasoned that out explicitly and
re-ran anyway.

So this reports the three facts that decision needs:

* **What the base gained** — the commits and the files they touched.
* **What this branch owns** — the files its own commits touch, i.e. since the
  merge base, not since some fixed point.
* **Where those two overlap** — the only files where the base's movement can
  interact with this branch's work, semantically or textually.

Plus the two predicates that say whether a gate can be skipped, delegated to
``review_diff`` so the exclude lists have one owner:

* ``runs_artifact_gates`` — did the base delta touch a **generation input**
  (``programs/**``, ``sdk/idl/**``, ``sdk/codama/**``, ``sdk/math-core/**``,
  ``sdk/interface/**``)? If not, no committed artifact can have gone stale.
* ``runs_rust_suites`` — did it touch anything outside CI's code filter?

And a ``rerun`` block that turns those facts into the per-gate verdicts the
caller used to derive by hand from the file lists — re-lint, rebuild the
program, reinstall frontend dependencies, re-run the artifact gates, and whether
a rebase-only re-run may enqueue before re-verifying — each a ``run`` boolean
with its ``reason``.

Usage::

    # Preferred: one call, BEFORE the rebase. The tool captures the merge-base,
    # fetches, and triages; the caller then rebases.
    python3 .claude/tools/rebase_overlap.py --base main
    git rebase origin/main

    # Explicit range, when the merge-base was captured by hand:
    python3 .claude/tools/rebase_overlap.py --from <prev> --to origin/main

``--base`` works before the rebase because nothing it measures depends on it:
the base delta is ``<merge-base>..origin/<base>`` either way, and the branch's
own files are ``<merge-base>..HEAD`` before the rebase and
``origin/<base>..HEAD`` after — the same commits, replayed.

Output is a summary by default — counts, ``overlap``, the predicates and the
``rerun`` block. The full ``base_files`` / ``branch_files`` / commit lists sit
behind ``--full``: they ran to 80-odd mostly-generated paths per call, were
replayed on every later turn, and were never read past ``overlap``.

**Pass the merge-base, not** ``git rev-parse origin/main``. ``origin/main`` is a
*shared* ref — worktrees have one ``.git``, so a sibling session's fetch can
advance it long before this runs. Reading it "before the fetch" can therefore
capture a tip this branch was never based on, and the comparison then reports a
0-commit delta for a base that demonstrably moved: a false all-clear, in exactly
the place a false all-clear licenses skipping a gate. (This is not hypothetical
— it happened on the tool's first real invocation.) The merge-base is what the
branch actually sits on, whoever fetched what.

``--from`` is required and deliberately so: once the rebase lands, the old
merge-base is no longer derivable from the branch, and guessing it (``ORIG_HEAD``
is the pre-rebase *branch* head, not the old base) would silently compare the
wrong range. ``--to`` defaults to ``origin/main``; ``--branch`` defaults to
``HEAD``.

**And ``--from`` must be an ancestor of ``--to``, which is checked.** The
warning above covers passing a base tip that *moved*; the opposite slip is
passing the **branch tip**, which makes the range a symmetric tree diff instead
of the base's movement. The branch's own new files then land in ``base_files``
and therefore in ``overlap`` — 42 "overlapping" files in one real session,
including ones only that branch had ever created, which momentarily read as the
branch having already merged. Both slips produce a *plausible* report, which is
why neither can be left to the reader to notice: this one is refused outright,
with the correct merge-base named in the error.

Prints JSON on stdout and a one-line human summary on stderr. Read-only apart
from ``--base``'s ``git fetch``: otherwise it runs only ``git merge-base``,
``git log`` and ``git diff --name-only``, and writes nothing.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_rebase_overlap.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import sys

from review_diff import (
    ReviewDiffError,
    _git,
    changed_files,
    touches_ci_code,
    touches_generation_input,
)


# A base change to any of these changes WHICH checks run or what they accept —
# a word dropped from the dictionary fails cspell on a file the delta never
# touched — so lint re-runs whatever the overlap.
LINT_CONFIG = frozenset(
    {
        "cfg/pre-commit-lint.yml",
        ".pre-commit-config.yaml",
        "cfg/dictionary.txt",
        "cfg/skill-size-baseline.json",
    }
)

# The committed optimized WASM binary: every interface-crate change rebuilds
# it, so on a hot SDK surface it overlaps on every base move.
WASM_BINARY = "sdk/ts/src/wasm/dropset_interface_bg.wasm"


def is_frontend_dep(path: str) -> bool:
    """The lockfile or a ``frontend/package*.json``: a base change to either
    leaves the worktree's ``node_modules`` stale, and an install earlier in the
    session does not count."""
    if path == "pnpm-lock.yaml":
        return True
    name = path.removeprefix("frontend/")
    return (
        name != path
        and "/" not in name
        and name.startswith("package")
        and name.endswith(".json")
    )


def commit_subjects(rev_range: str) -> list[str]:
    """``git log <range> --oneline`` as ``<sha> <subject>`` lines."""
    out = _git(["log", rev_range, "--oneline"])
    return [line for line in out.splitlines() if line.strip()]


def merge_base(a: str, b: str) -> str:
    """The merge base of two revisions."""
    return _git(["merge-base", a, b]).strip()


def resolve(rev: str) -> str:
    """``rev`` as a full commit sha, or raise if it names no commit."""
    return _git(["rev-parse", "--verify", f"{rev}^{{commit}}"]).strip()


def is_ancestor(maybe_ancestor: str, descendant: str) -> bool:
    """Whether ``maybe_ancestor`` is reachable from ``descendant``.

    Decided by **merge-base identity** rather than
    ``git merge-base --is-ancestor``, which answers through its *exit status*
    (0 yes, 1 no). ``_git`` raises on any non-zero exit, so that form cannot
    distinguish "no" from a bad revision without inspecting the status it has
    already discarded. The identity `merge-base(a, b) == a` holds exactly when
    ``a`` is an ancestor of ``b``, needs only the helper above, and lets a
    genuinely bad revision surface as the error it is.
    """
    return merge_base(maybe_ancestor, descendant) == resolve(maybe_ancestor)


def check_from_is_ancestor(previous_base: str, new_base: str) -> None:
    """Refuse a ``--from`` that is not an ancestor of ``--to``.

    The documented misuse is passing ``git rev-parse origin/main`` — a tip that
    moved. This guards the **opposite** slip, which the docstring above warns
    about in neither direction: passing the *branch tip* instead of the
    merge-base. That yields a symmetric tree diff rather than a
    base-movement diff, so the branch's own new files land in ``base_files``
    **and** therefore in ``overlap``. In one real session that read as 42
    overlapping files including ones only that branch had ever created, which
    momentarily looked like the branch had already merged.

    The failure mode is what makes the guard worth the call: the wrong answer is
    *plausible*. It is a well-formed report with a believable file count, and
    nothing in it says the range was backwards — so it is acted on. Refusing
    outright, and naming the merge-base the caller should have passed, is the
    only outcome that cannot be mistaken for a result.
    """
    if is_ancestor(previous_base, new_base):
        return
    correct = merge_base(previous_base, new_base)
    raise ReviewDiffError(
        f"--from {previous_base} is not an ancestor of --to {new_base}, so the "
        f"range would be a symmetric tree diff rather than the base's movement "
        f"— the branch's own files would appear in `base_files` and `overlap`. "
        f"Their merge-base is {correct}; pass that (capture it with "
        f"`git merge-base HEAD <base>` BEFORE the fetch/rebase)."
    )


def analyze(previous_base: str, new_base: str, branch: str = "HEAD") -> dict:
    """Compare a base's movement against a branch's own changes.

    ``branch_files`` is measured from the **merge base** of ``branch`` and
    ``new_base``, not from ``previous_base``: after a rebase the branch's commits
    sit on top of the new base, so a ``previous_base..branch`` range would fold
    the base's own movement into the branch's file set and report overlap
    everywhere.
    """
    base_files = changed_files(f"{previous_base}..{new_base}")
    commits = commit_subjects(f"{previous_base}..{new_base}")
    branch_files = changed_files(f"{merge_base(branch, new_base)}..{branch}")

    overlap = sorted(set(base_files) & set(branch_files))
    result = {
        "previous_base": previous_base,
        "new_base": new_base,
        "base_commits": len(commits),
        "base_commit_subjects": commits,
        "base_files": base_files,
        "branch_files": branch_files,
        "overlap": overlap,
        # Predicates over the BASE DELTA only — the question is whether the
        # base's movement could have staled an artifact this branch already
        # regenerated, not whether this branch touched one. Both are `review_diff`
        # functions, not re-implementations, so the logic has one owner as well
        # as the exclude lists.
        "runs_artifact_gates": touches_generation_input(base_files),
        "runs_rust_suites": touches_ci_code(base_files),
    }
    result["rerun"] = rerun_verdicts(result)
    return result


def _verdict(run: bool, reason: str) -> dict:
    return {"run": run, "reason": reason}


def rerun_verdicts(result: dict) -> dict:
    """What a rebase forces the caller to re-run, each with its reason.

    The rebase and every freshness gate always run; only these re-runs vary.
    Each rule is one the review used to apply by eye to ``base_files``, and
    each incident behind it is a ``review-pr`` ``history.md`` entry:

    * ``lint`` — a non-empty overlap re-runs it, ``review_diff.py`` included
      ("Empty-overlap re-runs"), and so does a base change to a ``LINT_CONFIG``
      file ("Lint-config base delta"). An empty overlap asserts the earlier
      result; a cross-file check (clippy, rustdoc, tsc) flipped by an API the
      base changed is the residual the merge queue's re-run catches.
    * ``program`` — a base change under ``programs/`` leaves the built ``.so``
      stale, and a scoped ``cargo test`` against it reports the base's changes
      as this branch's regressions ("Stale program after rebase").
    * ``frontend_install`` — see ``is_frontend_dep`` ("Stale node_modules
      after rebase").
    * ``artifact_gates`` — ``runs_artifact_gates``, restated as a verdict;
      an overlap on the committed WASM binary names the drop → rebase →
      regenerate sequence instead ("WASM binary overlap").
    * ``may_enqueue_unverified`` — a re-run forced only by a rebase may push and
      enqueue and let the queue verify, when the overlap is empty: the queue
      runs the whole suite against the merged result whatever the paths, so it
      covers any delta ("Rebase race"). The other condition — the branch's own
      diff unchanged since its last green local pass — is the caller's to
      check; the tool cannot see it.
    """
    base_files = result["base_files"]
    overlap = result["overlap"]

    lint_config = sorted(LINT_CONFIG.intersection(base_files))
    if lint_config:
        lint = _verdict(True, f"base changed the lint config: {', '.join(lint_config)}")
    elif overlap:
        lint = _verdict(True, f"{len(overlap)} file(s) overlap this branch")
    else:
        lint = _verdict(False, "overlap empty — earlier lint results still stand")

    programs = [f for f in base_files if f.startswith("programs/")]
    if programs:
        program = _verdict(
            True,
            f"base touched {len(programs)} file(s) under programs/ — "
            "`make program` before any suite",
        )
    else:
        program = _verdict(False, "base touched nothing under programs/")

    deps = [f for f in base_files if is_frontend_dep(f)]
    if deps:
        frontend = _verdict(
            True,
            f"base changed {', '.join(deps)} — install before any typecheck or lint",
        )
    else:
        frontend = _verdict(False, "base changed no frontend dependency manifest")

    if WASM_BINARY in overlap:
        # Git cannot merge it and `wasm-opt` is not reproducible, so the only
        # correct content is a rebuild from the merged tree ("WASM binary
        # overlap").
        reason = (
            f"{WASM_BINARY} overlaps — drop any `Rebuild WASM bindings` "
            "commit, rebase, then let `regen_artifacts.py` rebuild it "
            "(`make wasm`, recommitted under that subject)"
        )
        if result["runs_artifact_gates"]:
            reason += "; base delta also touched a generation input"
        gates = _verdict(True, reason)
    elif result["runs_artifact_gates"]:
        gates = _verdict(True, "base delta touched a generation input")
    else:
        gates = _verdict(False, "base delta touched no generation input")

    if overlap:
        enqueue = _verdict(False, "overlap non-empty — verify locally")
    else:
        enqueue = _verdict(
            True,
            "overlap empty and the queue re-runs the whole suite — provided the "
            "branch's own diff is unchanged since its last green local pass",
        )

    return {
        "lint": lint,
        "program": program,
        "frontend_install": frontend,
        "artifact_gates": gates,
        "may_enqueue_unverified": enqueue,
    }


def compact(result: dict) -> dict:
    """The default output: the full lists replaced by their counts.

    The decision reads ``overlap``, the commit count, the predicates and the
    ``rerun`` block; the lists themselves were 60–90 mostly-generated paths per
    call that nothing read, replayed as input on every later turn.
    """
    out = {
        k: v
        for k, v in result.items()
        if k not in ("base_commit_subjects", "base_files", "branch_files")
    }
    out["base_file_count"] = len(result["base_files"])
    out["branch_file_count"] = len(result["branch_files"])
    return out


def capture_and_fetch(base: str, branch: str = "HEAD") -> tuple[str, str]:
    """Capture the branch's merge-base with ``origin/<base>``, then fetch.

    Returns ``(previous_base, new_base)``. The merge-base is read **before**
    the fetch — the tool owns the ordering, so no caller can get it backwards.
    """
    tracking = f"origin/{base}"
    previous = merge_base(branch, tracking)
    _git(["fetch", "origin", base])
    return previous, tracking


def last_head_action(branch: str = "HEAD") -> str:
    """The subject of ``branch``'s newest reflog entry, or ``""``."""
    try:
        return _git(["reflog", "-1", "--format=%gs", branch]).strip()
    except ReviewDiffError:
        return ""


def just_rebased_warning(result: dict, base: str, branch: str = "HEAD") -> str | None:
    """A warning when a ``--base`` report may be a post-rebase false all-clear.

    Run AFTER a rebase onto the fetched tip, the branch's merge-base IS that
    tip, so the triage reports a 0-commit delta for a base that moved — the
    plausible false all-clear ``check_from_is_ancestor`` refuses in the
    ``--from`` mode. That shape (newest reflog entry a rebase, 0 commits) is
    also what an honest later re-check looks like when nothing moved, so this
    warns rather than refusing, and names the ``--from`` that triages the
    rebase from ``ORIG_HEAD``, the pre-rebase branch.

    The residual it cannot see: a rebase not triaged first, followed by the base
    moving AGAIN before ``--base`` runs reports the new commits only — a
    partial all-clear with no 0-commit signature. Running ``--base`` before
    every rebase, as the skill orders, is the guard; this is the backstop.
    """
    # `rebase (finish): …`, and `pull --rebase (finish): …` for a pull.
    action = last_head_action(branch).split(":", 1)[0]
    if result["base_commits"] or "rebase" not in action:
        return None
    try:
        previous = merge_base("ORIG_HEAD", f"origin/{base}")
        hint = f"--from {previous} --to origin/{base}"
    except ReviewDiffError:
        hint = f"--from <the pre-rebase merge-base> --to origin/{base}"
    return (
        "HEAD's last action was a rebase and the delta is 0 commits: if that "
        "rebase was not triaged before it ran, this is NOT an all-clear — "
        f"triage it with {hint}"
    )


def summarize(result: dict) -> str:
    """One human line: what moved, what it can stale, and where it collides."""
    parts = [
        f"rebase-overlap | base gained {result['base_commits']} commit(s) "
        f"touching {len(result['base_files'])} file(s)"
    ]
    if result["overlap"]:
        parts.append(f"{len(result['overlap'])} overlap with this branch")
    else:
        parts.append("no overlap with this branch")

    gates = []
    if result["runs_artifact_gates"]:
        gates.append("artifact gates")
    if result["runs_rust_suites"]:
        gates.append("Rust suites")
    if gates:
        parts.append("base delta can stale: " + ", ".join(gates))
    else:
        # The whole point of the tool: name the skip explicitly rather than
        # leaving a reader to infer it from two empty lists.
        parts.append(
            "base delta touches no generation input and nothing outside CI's "
            "code filter — assert the gates once rather than re-running them"
        )
    verdicts = dict(result.get("rerun", {}))
    # A permission, not a re-run, so it is said apart from the list.
    enqueue = verdicts.pop("may_enqueue_unverified", None)
    reruns = [name for name, v in verdicts.items() if v["run"]]
    if reruns:
        parts.append("re-run: " + ", ".join(reruns))
    if enqueue and enqueue["run"]:
        parts.append("a rebase-only re-run may enqueue unverified")
    return " | ".join(parts)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="rebase_overlap.py")
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument(
        "--base",
        help="the PR's base branch: capture the merge-base with origin/<base>, "
        "fetch it, and triage — run BEFORE the rebase",
    )
    source.add_argument(
        "--from",
        dest="previous_base",
        help="the base the branch was on: git merge-base HEAD <base>, "
        "captured before the fetch/rebase (NOT git rev-parse origin/<base>)",
    )
    parser.add_argument(
        "--to",
        dest="new_base",
        default=None,
        help="the base tip after (with --from; default origin/main)",
    )
    parser.add_argument("--branch", default="HEAD", help="the branch under review")
    parser.add_argument(
        "--full",
        action="store_true",
        help="include the full base_files / branch_files / commit lists",
    )
    args = parser.parse_args(argv[1:])

    if args.base:
        if args.new_base is not None:
            parser.error("--to applies to --from; --base always means origin/<base>")
        previous_base, new_base = capture_and_fetch(args.base, args.branch)
    else:
        previous_base, new_base = args.previous_base, args.new_base or "origin/main"
    check_from_is_ancestor(previous_base, new_base)
    result = analyze(previous_base, new_base, args.branch)
    if args.base:
        warning = just_rebased_warning(result, args.base, args.branch)
        if warning:
            result["warning"] = warning
            print(f"rebase-overlap: WARNING: {warning}", file=sys.stderr)
    json.dump(result if args.full else compact(result), sys.stdout, indent=2)
    print()
    print(summarize(result), file=sys.stderr)
    return 0


def main() -> int:
    # Every failure path here originates in `review_diff._git`, so
    # `ReviewDiffError` is the only exception this module can surface. It
    # deliberately defines no error type of its own: an unraised, unreachable
    # one would just be dead code with a matching dead `except` arm.
    try:
        return run(sys.argv)
    except ReviewDiffError as exc:
        print(f"rebase-overlap: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
