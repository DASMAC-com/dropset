#!/usr/bin/env python3
"""``make lint`` file-set resolver — run pre-commit over every file in the
working tree, **including the ones git doesn't track yet**.

``pre-commit run --all-files`` sounds exhaustive but isn't: it enumerates its
file list with ``git ls-files``, which sees only what is **in the index**. A
brand-new file that has never been ``git add``ed is not in that list, so no hook
ever looks at it — cspell, rustfmt, biome, yamllint, mdformat, all of them. The
run reports a cheerful ``Passed`` for a file it never opened.

That is a real, load-bearing divergence from CI, because CI checks out a branch
where the file **is** committed and therefore **is** tracked. The observed shape
(PR #308): a new venue module was written, ``make lint`` passed locally, and the
CI Lint job failed on cspell violations in that same file. Nothing about the
command or the config differed — the only variable was whether the path had
reached the index. Reproduced deterministically on a clean tree with a fixture
holding one nonsense word: untracked ⇒ ``cspell...Passed``; ``git add`` the
byte-identical file ⇒ ``cspell...Failed``.

So this tool builds the file list itself:

    git ls-files --cached --others --exclude-standard -z

— tracked **plus** untracked-but-not-ignored — filters it to paths that still
exist on disk, and hands the result to ``pre-commit run --files …``. The local
set is then a **superset** of CI's, which is the safe direction: a violation
that CI would catch is caught locally first, on the same tree, and reported as
the actual violation rather than as a "you forgot to stage something" nag.

Deliberately **not** applied to the CI workflow, which stays on ``--all-files``.
CI's checkout is clean, so for tracked files the two are identical there — but
CI runs ``make decks-build`` before linting, and that step leaves build output
in the tree. Under ``--all-files`` those artifacts are invisible (untracked);
under this tool they would suddenly be linted. Superset locally, unchanged in
CI.

**Do not "optimize" this back to ``--all-files``.** The chunked, parallel cspell
batches visible in the output (``Files checked: 36`` repeated) are pre-commit's
ordinary xargs partitioning for concurrency — they were once suspected of hiding
the violation and they do not: every chunk runs, and the chunk holding a bad
file reports it. Chunking is not the bug; the file list is.

``--changed`` narrows that list to **this branch's own work** — everything that
differs from the merge-base with ``origin/main``, plus every
untracked-not-ignored file. It exists to make the *cheap* check the *easy* one.
Both `review-pr` and `init-pr` already prescribe a scoped re-run after an edit,
with the full sweep reserved for the checkpoints — and both are routinely
ignored, because at the moment of typing ``make lint`` is the shape in muscle
memory while the scoped form
(``pre-commit run <hook> --config … --files <paths…>``) needs the changed-file
list assembled by hand. One measured session paid **13 full sweeps (≈5.8k)**
while editing the very rule that forbids them. So this is not new machinery:
the tool already owns the file list, and this is a filter over it that collapses
the scoped form to one bare command.

Note the cost being avoided is **context, in failure tails** — the bytes are
hook output on a fail, so wrapping a full sweep in ``run_quiet.py`` does nothing
(it already is wrapped). The only lever is *fewer full sweeps*.

``--files <paths…>`` lints exactly the named paths, so a narrower-than-branch
check needs no hand-spelled ``pre-commit run --config … --files …`` (whose
``--config`` is mandatory and whose default name fails with
``InvalidConfigError``).

Two failure shapes are handled here rather than in prose:

* **A fix-then-fail hook** (``mdformat``, ``ruff format``, ``biome``, the
  dictionary sorter …) rewrites a file and fails by design. When the run fails
  and pre-commit says a hook modified files, the tool re-runs **once** and
  reports only that second result — a second failure is the real one.
* **A missing ``biome`` / ``tsc``.** Both run through the frontend's
  ``node_modules``; when a JS/TS/CSS path is in the set and the binary is
  absent, the tool installs the frontend dependencies first. If the install
  fails, those hooks are skipped and reported **unverifiable** — CI runs them —
  so the remaining hooks still return a verdict.

Stdlib only. This is a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys

# The lint hook set. Matches the `--config` CI's Lint job passes, so the two
# runs differ only in their file list (and this tool's list is the superset).
DEFAULT_CONFIG = "cfg/pre-commit-lint.yml"

# What `--changed` measures against. The **merge-base** with this ref, never the
# ref itself: diffing against the tip reports the base's own commits as changes
# too, which is the same symmetric-diff trap `rebase_overlap.py` documents.
DEFAULT_BASE = "origin/main"

# Tracked (`--cached`) plus untracked-but-not-gitignored (`--others` with
# `--exclude-standard`), NUL-separated so a path containing a space — or, in
# principle, a newline — survives the round trip.
_LS_FILES = [
    "git",
    "ls-files",
    "--cached",
    "--others",
    "--exclude-standard",
    "-z",
]

# The hook `--prose` runs. Named here rather than inline so the pre-flight and
# the hook config cannot drift apart silently.
PROSE_HOOK = "cspell"

# `--changed`'s untracked half. A file that has never been added is by
# definition part of this branch's work, and it is exactly the class the
# whole-tree resolver above exists to stop losing — so it must survive the
# narrowing too, not just the full sweep.
_LS_OTHERS = [
    "git",
    "ls-files",
    "--others",
    "--exclude-standard",
    "-z",
]


# pre-commit's own line for a hook that rewrote a file.
MODIFIED_MARKER = "files were modified by this hook"

# The hooks that run through `frontend/node_modules`, and the suffixes that
# select them (the union of their `types_or`, plus the module variants).
FRONTEND_HOOKS = ("biome", "tsc")
FRONTEND_SUFFIXES = (".css", ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx")

_FRONTEND_INSTALL = ["pnpm", "--dir", "frontend", "install"]


def repo_root() -> str:
    """Return the working tree's top-level directory.

    ``git ls-files`` reports paths relative to the **current** directory, and
    pre-commit insists on running from the root anyway, so both subprocesses are
    pinned here rather than trusting the caller's cwd.
    """
    return subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def parse_ls_files(raw: str) -> list[str]:
    """Split NUL-separated ``git ls-files -z`` output into a deduped, sorted
    path list.

    Dedup matters because ``--cached --others`` can report the same path twice
    (an unmerged entry, say); handing pre-commit a duplicate would lint it
    twice for no benefit.
    """
    return sorted({path for path in raw.split("\0") if path})


def existing(paths: list[str], root: str) -> list[str]:
    """Drop paths that aren't present in the working tree.

    A file deleted from the worktree but still in the index is listed by
    ``--cached``, and passing pre-commit a path that isn't there makes hooks
    fail on a missing file rather than on anything real.

    Uses ``lexists``, so a symlink counts as present even when its target is
    gone — a tracked symlink is in CI's list too, and silently dropping it here
    would reopen the very gap this tool closes.
    """
    return [p for p in paths if os.path.lexists(os.path.join(root, p))]


def pre_commit_cmd(config: str, hook_args: list[str], files: list[str]) -> list[str]:
    """Build the ``pre-commit run`` argv.

    Order matters and is easy to get subtly wrong: a hook id is a **positional**
    argument, so anything forwarded by the caller has to land before the
    ``--files`` list — once ``--files`` starts consuming, every remaining token
    is a path.
    """
    return ["pre-commit", "run", "--config", config, *hook_args, "--files", *files]


def lint_files(root: str) -> list[str]:
    """Resolve the full set of files ``make lint`` should check, under ``root``."""
    raw = subprocess.run(
        _LS_FILES,
        capture_output=True,
        text=True,
        check=True,
        cwd=root,
    ).stdout
    return existing(parse_ls_files(raw), root)


def merge_base(root: str, base_ref: str) -> str:
    """Return the merge-base of ``HEAD`` and ``base_ref``.

    Raises ``subprocess.CalledProcessError`` when the ref is unknown — a
    missing ``origin/main`` means the caller has not fetched, and silently
    falling back to the whole tree would turn a stale-base mistake into a
    surprise full sweep.

    Git's own stderr is **echoed and attached** rather than dropped.
    ``capture_output=True`` takes it off the terminal, and
    ``CalledProcessError`` renders as nothing but "returned non-zero exit
    status 128" — so the operator would learn that a ref lookup failed but not
    *which* ref or why, which is the entire diagnostic content. Echoing is what
    actually surfaces it (the attribute alone does not print, whoever catches
    it); the ``stderr=`` attachment is for a programmatic caller.
    """
    proc = subprocess.run(
        ["git", "merge-base", "HEAD", base_ref],
        capture_output=True,
        text=True,
        cwd=root,
    )
    if proc.returncode != 0:
        detail = proc.stderr.strip() or f"git merge-base HEAD {base_ref} failed"
        print(f"lint-paths: {detail}", file=sys.stderr)
        raise subprocess.CalledProcessError(
            proc.returncode,
            proc.args,
            output=proc.stdout,
            stderr=detail,
        )
    return proc.stdout.strip()


def changed_files(root: str, base_ref: str = DEFAULT_BASE) -> list[str]:
    """Resolve just this branch's own files: everything differing from the
    merge-base with ``base_ref``, plus every untracked-not-ignored path.

    ``git diff <commit>`` compares that commit to the **working tree**, so
    unstaged edits are included — which is what a post-edit check wants, since
    the edit being checked has usually not been staged yet.
    """
    base = merge_base(root, base_ref)
    tracked = subprocess.run(
        ["git", "diff", "--name-only", "-z", base, "--"],
        capture_output=True,
        text=True,
        check=True,
        cwd=root,
    ).stdout
    untracked = subprocess.run(
        _LS_OTHERS,
        capture_output=True,
        text=True,
        check=True,
        cwd=root,
    ).stdout
    return existing(parse_ls_files(tracked + "\0" + untracked), root)


def missing_frontend_hooks(
    root: str, hook_args: list[str], files: list[str]
) -> list[str]:
    """The frontend hooks this run would select whose binary is absent.

    Empty when no JS/TS/CSS path is in the set, or when ``hook_args`` names a
    hook that is not a frontend one.
    """
    if not any(f.endswith(FRONTEND_SUFFIXES) for f in files):
        return []
    selected = [h for h in FRONTEND_HOOKS if not hook_args or h in hook_args]
    bin_dir = os.path.join(root, "frontend", "node_modules", ".bin")
    return [h for h in selected if not os.path.exists(os.path.join(bin_dir, h))]


def ensure_frontend_hooks(
    root: str, hook_args: list[str], files: list[str]
) -> list[str]:
    """Install the frontend dependencies if a selected hook's binary is absent.

    Returns the hooks that still cannot run afterwards — the ones to skip and report
    unverifiable. The install's output is captured: on success it is noise, and
    on failure its last lines are all the diagnosis the caller needs.
    """
    missing = missing_frontend_hooks(root, hook_args, files)
    if not missing:
        return []
    print(
        f"lint-paths: {', '.join(missing)} not installed — "
        f"running `{' '.join(_FRONTEND_INSTALL)}`",
        file=sys.stderr,
    )
    proc = subprocess.run(_FRONTEND_INSTALL, cwd=root, capture_output=True, text=True)
    if proc.returncode != 0:
        tail = (proc.stderr or proc.stdout).strip().splitlines()[-5:]
        for line in tail:
            print(f"lint-paths:   {line}", file=sys.stderr)
    return missing_frontend_hooks(root, hook_args, files)


def run_pre_commit(
    cmd: list[str], root: str, env: dict | None = None
) -> tuple[int, bool]:
    """Run pre-commit, mirroring its output, and say whether a hook rewrote a
    file. Returns ``(exit code, modified)``."""
    modified = False
    with subprocess.Popen(
        cmd,
        cwd=root,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    ) as proc:
        assert proc.stdout is not None
        for line in proc.stdout:
            sys.stdout.write(line)
            modified = modified or MODIFIED_MARKER in line
    sys.stdout.flush()
    return proc.returncode, modified


def lint(config: str, hook_args: list[str], files: list[str], root: str) -> int:
    """Run the hooks over ``files``, absorbing the two by-design failures."""
    env = None
    unverifiable = ensure_frontend_hooks(root, hook_args, files)
    if unverifiable:
        skip = [s for s in os.environ.get("SKIP", "").split(",") if s]
        env = {**os.environ, "SKIP": ",".join(skip + unverifiable)}
    cmd = pre_commit_cmd(config, hook_args, files)

    code, modified = run_pre_commit(cmd, root, env)
    if code != 0 and modified:
        print(
            "lint-paths: a hook rewrote files — re-running once; only a second "
            "failure is real",
            file=sys.stderr,
        )
        code, _ = run_pre_commit(cmd, root, env)

    if unverifiable:
        print(
            f"lint-paths: UNVERIFIABLE locally: {', '.join(unverifiable)} "
            "(frontend install failed) — CI runs them; do not gate on them",
            file=sys.stderr,
        )
    return code


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="lint_paths.py",
        description=(
            "Run pre-commit over every file in the working tree, including "
            "untracked ones that `--all-files` silently skips, or with "
            "`--changed` over just this branch's own files. Extra arguments "
            "after `--` are forwarded to pre-commit (e.g. a single hook id)."
        ),
    )
    parser.add_argument(
        "--config",
        default=DEFAULT_CONFIG,
        help=f"the pre-commit config to run (default: {DEFAULT_CONFIG})",
    )
    scope = parser.add_mutually_exclusive_group()
    scope.add_argument(
        "--changed",
        action="store_true",
        help="lint only this branch's own files — everything differing from "
        "the merge-base with the base ref, plus untracked-not-ignored paths. "
        "The post-edit check; the full sweep stays for the checkpoints.",
    )
    scope.add_argument(
        "--files",
        nargs="+",
        metavar="PATH",
        help="lint exactly these paths (relative to the repo root); end the "
        "list with `--` before any hook id",
    )
    parser.add_argument(
        "--base",
        default=DEFAULT_BASE,
        help=f"the ref --changed takes its merge-base against "
        f"(default: {DEFAULT_BASE})",
    )
    parser.add_argument(
        "--prose",
        action="store_true",
        help="the spelling pre-flight: run ONLY the cspell hook over this "
        "branch's changed files. Implies --changed, and takes no arguments, "
        "which is the whole point — see PROSE_HOOK.",
    )
    parser.add_argument(
        "--print",
        action="store_true",
        dest="print_only",
        help="print the resolved file list, one path per line, and exit "
        "without running pre-commit",
    )
    parser.add_argument(
        "hook_args",
        nargs="*",
        help="arguments forwarded verbatim to `pre-commit run` (pass them after `--`)",
    )
    args = parser.parse_args(argv)

    hook_args = list(args.hook_args)
    if args.prose:
        # `--prose` is a zero-argument spelling pre-flight, and being
        # zero-argument is the mechanism rather than a convenience. The rule
        # "check spelling before the first full round" exists, is documented,
        # and gets skipped — because the full sweep needs no arguments while
        # the prescribed narrow form needs a hook id and a path list. That is
        # the same asymmetry `--changed` was added to remove, one level down.
        #
        # What it buys is FEWER FAILED ROUNDS, not narrower output: one
        # measured session ran `make lint` six times for ~2.1k of failure
        # tails, two of those rounds spelling-only, each discovering three
        # more British spellings. The scoped form
        # would have failed identically on an unknown word; only finding them
        # all at once helps.
        args.changed = True
        if not hook_args:
            hook_args = [PROSE_HOOK]

    root = repo_root()
    if args.files:
        files = existing(sorted(set(args.files)), root)
        if not files:
            print("lint-paths: none of the --files paths exist", file=sys.stderr)
            return 1
    elif args.changed:
        files = changed_files(root, args.base)
    else:
        files = lint_files(root)

    if args.print_only:
        for path in files:
            print(path)
        return 0

    # An empty **changed** set is a legitimate answer — a branch with nothing
    # to check yet, or one whose only edits were reverted — so it reports
    # success. The whole-tree case below cannot say that.
    if args.changed and not files:
        print(f"lint-paths: nothing changed vs the merge-base with {args.base}")
        return 0

    # Deliberately non-zero. An empty set means the resolver found nothing to
    # check, and reporting success for a run that opened no files is the exact
    # failure this tool exists to remove — just arrived at from the other end.
    # A working tree always has files, so this fires only when something is
    # wrong (a bad cwd, a future refactor of the resolver), and it should fire
    # loudly rather than hand back a green lint.
    if not files:
        print(
            "lint-paths: resolved zero files to lint — refusing to report success",
            file=sys.stderr,
        )
        return 1

    return lint(args.config, hook_args, files, root)


if __name__ == "__main__":
    raise SystemExit(main())
