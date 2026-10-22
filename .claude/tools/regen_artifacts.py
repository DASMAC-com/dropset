#!/usr/bin/env python3
"""Regenerate the committed generated artifacts and report each family's state.

``review-pr`` step 9 mirrors four CI gates that fail a PR on a committed file
stale against its source — the **IDL** (``test.yml``), and the **SDK
clients**, **conformance vectors** and **committed WASM glue** (``sdk.yml``).
Each was the same run-stage-diff-commit sequence typed out by hand, four times,
with two failure modes left to prose: a toolchain check aborting the target,
and a build that looks hung because another worktree holds the cargo lock. Per
``docs/conventions/skill-tooling.md`` that settled shape lives here, and the
skill reads one verdict.

For each family, in dependency order (the IDL before the clients built from
it), the tool runs the ``make`` target with its output captured to a log
(``run_quiet.py``'s capture, lock-wait echo included), stages the family's
paths, and commits any diff signed with the canonical message. Each family
reports one ``status``:

* ``fresh`` — the target ran and nothing moved;
* ``committed`` — it moved, and the commit is named;
* ``unverifiable`` — a toolchain check (``check-toolchain`` / ``check-wasm`` /
  ``check-pnpm``) aborted the target, so the gate cannot be checked locally.
  The committed artifact stands, and this is **never a pass**: the exit status
  says so, and the summary must name it;
* ``failed`` — the target or the commit failed for any other reason; the log
  names why;
* ``skipped`` — the path gate found no generation input in the branch.

**The path gate** is ``review_diff.py``'s, over the same inputs: a branch
whose non-docs paths touch no generation input cannot have staled any artifact,
so every family is ``skipped`` without a build. ``--force`` runs them anyway.

Usage::

    python3 .claude/tools/regen_artifacts.py [--base main] [--force]

Prints JSON::

    {
      "runs_artifact_gates": true,     // the path gate's answer
      "forced": false,
      "families": [
        {"family": "idl", "target": "make idl", "status": "committed",
         "commit": "abc1234", "log": "/…/make-idl-123.log", "detail": null},
        …
      ],
      "lint_rerun": true,              // an artifact commit landed: re-lint
      "verified": true                 // every family fresh, committed or skipped
    }

Exit 0 when every family is fresh, committed or skipped; 1 when one failed; 3
when none failed but one is unverifiable; 2 on a usage error.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_regen_artifacts.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import collections
import contextlib
import json
import os
import re
import subprocess
import sys

import review_diff
import run_quiet

#: One committed generated artifact: the target that rebuilds it, the paths it
#: writes, and the message its refresh is committed under.
Family = collections.namedtuple("Family", "name cmd paths message")

#: In dependency order. The clients are generated FROM the IDL, so a stale IDL
#: has to be rebuilt first or the client gate compares against the old one. The
#: vectors target is the generator itself, not `check-conformance-vectors`:
#: staging and diffing is this tool's job, uniformly across the four.
FAMILIES = (
    Family("idl", ["make", "idl"], ["sdk/idl/dropset.json"], "Rebuild IDL"),
    Family(
        "sdk",
        ["make", "sdk"],
        ["sdk/ts/src/generated", "sdk/rs/src/generated"],
        "Regenerate SDK clients",
    ),
    Family(
        "vectors",
        ["make", "conformance-vectors"],
        ["sdk/conformance"],
        "Regenerate conformance vectors",
    ),
    Family("wasm", ["make", "wasm"], ["sdk/ts/src/wasm"], "Rebuild WASM bindings"),
)

#: The lines the Makefile's toolchain checks print before `exit 1`. A target
#: that aborts on one never reached the generator, so its gate is unverifiable
#: rather than failed.
TOOLCHAIN_ABORT_RE = re.compile(
    r"^(anchor-cli \S+ required"
    r"|cargo build-sbf not found"
    r"|solana-(?:cli|test-validator) \S+ or later required"
    r"|wasm-pack not found"
    r"|pnpm not found)"
)

EXIT_OK = 0
EXIT_FAILED = 1
EXIT_USAGE = 2
EXIT_UNVERIFIABLE = 3


def _git(args, check=True):
    return subprocess.run(["git", *args], capture_output=True, text=True, check=check)


def runs_artifact_gates(base: str) -> bool:
    """``review_diff.py``'s artifact path gate, over the branch against ``base``.

    The predicate and the docs-path carve-out are review_diff's, imported rather
    than restated, so the two tools cannot disagree about whether a gate runs.
    """
    paths = review_diff.changed_files(f"origin/{base}...HEAD")
    source = [p for p in paths if review_diff.slice_for(p) != "docs"]
    return review_diff.touches_generation_input(source)


def run_target(cmd) -> tuple[int, str, bool]:
    """Run ``cmd`` with its output captured. Returns (exit, log path, aborted).

    ``aborted`` is True when a toolchain check's message is in the log. The
    lock-wait echo goes to stderr, so stdout stays one JSON document.
    """
    os.makedirs(run_quiet.LOG_DIR, mode=0o700, exist_ok=True)
    log_path = os.path.join(
        run_quiet.LOG_DIR, "%s-%d.log" % (run_quiet.sanitize(cmd), os.getpid())
    )
    try:
        with open(log_path, "w", encoding="utf-8", errors="replace") as log:
            os.chmod(log_path, 0o600)
            with contextlib.redirect_stdout(sys.stderr):
                code, _ = run_quiet.stream_to_log(cmd, log)
    except OSError as exc:
        with open(log_path, "a", encoding="utf-8") as log:
            log.write("could not launch %s: %s\n" % (cmd[0], exc))
        return run_quiet.LAUNCH_FAILURE_CODE, log_path, False
    with open(log_path, encoding="utf-8", errors="replace") as log:
        aborted = any(TOOLCHAIN_ABORT_RE.match(line) for line in log)
    return code, log_path, aborted


def commit_family(family: Family, sign: bool) -> tuple[str | None, str | None]:
    """Stage ``family``'s paths and commit any diff. Returns (sha, error).

    ``(None, None)`` means nothing moved. A commit the pre-commit hooks reject
    is re-staged and retried once, since a whitespace or end-of-file fixer
    rewrites the file and fails the first attempt by design. A git error (a
    held index lock, a path that matches nothing) is reported, not raised, so
    one family's failure still leaves a JSON verdict.
    """
    try:
        return _commit_family(family, sign)
    except subprocess.CalledProcessError as exc:
        return None, "git %s failed: %s" % (exc.cmd[1], (exc.stderr or "").strip())


def _commit_family(family: Family, sign: bool) -> tuple[str | None, str | None]:
    _git(["add", "-A", "--", *family.paths])
    if (
        _git(
            ["diff", "--cached", "--quiet", "--", *family.paths], check=False
        ).returncode
        == 0
    ):
        return None, None
    commit = ["commit", "-q", "-m", family.message, "--", *family.paths]
    if sign:
        commit.insert(1, "-S")
    result = _git(commit, check=False)
    if result.returncode != 0:
        _git(["add", "-A", "--", *family.paths])
        result = _git(commit, check=False)
    if result.returncode != 0:
        tail = (result.stderr or result.stdout).strip().splitlines()[-5:]
        return None, "commit failed: " + " / ".join(tail)
    return _git(["rev-parse", "--short", "HEAD"]).stdout.strip(), None


def regenerate(families=FAMILIES, sign: bool = True) -> list[dict]:
    """Run every family in order and return one report entry per family.

    A family that fails does not stop the rest: each gate is independent in CI,
    and an unverifiable IDL still leaves the clients checkable against the
    committed one.
    """
    report = []
    for family in families:
        entry = {
            "family": family.name,
            "target": " ".join(family.cmd),
            "status": None,
            "commit": None,
            "log": None,
            "detail": None,
        }
        code, entry["log"], aborted = run_target(family.cmd)
        if code != 0:
            entry["status"] = "unverifiable" if aborted else "failed"
            entry["detail"] = (
                "a toolchain check aborted the target"
                if aborted
                else ("exit %d; inspect the log" % code)
            )
        else:
            sha, error = commit_family(family, sign)
            entry["commit"] = sha
            entry["detail"] = error
            entry["status"] = "failed" if error else ("committed" if sha else "fresh")
        report.append(entry)
    return report


def skipped(families=FAMILIES) -> list[dict]:
    return [
        {
            "family": f.name,
            "target": " ".join(f.cmd),
            "status": "skipped",
            "commit": None,
            "log": None,
            "detail": "the branch touches no generation input",
        }
        for f in families
    ]


def exit_status(report: list[dict]) -> int:
    statuses = {entry["status"] for entry in report}
    if "failed" in statuses:
        return EXIT_FAILED
    if "unverifiable" in statuses:
        return EXIT_UNVERIFIABLE
    return EXIT_OK


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog="regen_artifacts.py")
    parser.add_argument("--base", default="main", help="base branch (default main)")
    parser.add_argument(
        "--force",
        action="store_true",
        help="run every family even when no generation input changed",
    )
    args = parser.parse_args(argv)
    if args.base.startswith("-"):
        print(f"error: invalid base branch name: {args.base!r}", file=sys.stderr)
        return EXIT_USAGE

    gates = runs_artifact_gates(args.base)
    report = regenerate() if gates or args.force else skipped()
    json.dump(
        {
            "runs_artifact_gates": gates,
            "forced": args.force,
            "families": report,
            "lint_rerun": any(e["status"] == "committed" for e in report),
            "verified": exit_status(report) == EXIT_OK,
        },
        sys.stdout,
        indent=2,
    )
    sys.stdout.write("\n")
    return exit_status(report)


if __name__ == "__main__":
    sys.exit(main())
