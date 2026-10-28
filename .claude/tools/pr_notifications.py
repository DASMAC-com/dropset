#!/usr/bin/env python3
"""PR notification dismissal — the one rule every caller applies, instead of
the two that drifted (upkeep dismissed only behind a Linear-closed gate, the
queue watcher on merged only, and the close paths never).

Usage:

    python3 .claude/tools/pr_notifications.py dismiss --pr 663 [--pr 664] [--apply]
    python3 .claude/tools/pr_notifications.py dismiss --settled [--apply]

``--pr`` selects every thread on the named PRs, whatever their state — the
close path's form, which has already refused an open PR. ``--settled`` selects
every thread whose PR is merged or closed, whatever its issue's Linear state:
a merged PR whose issue sits In Review is settled code, and its threads are
the bulk of the inbox. An open PR's threads are never selected by it.

A dry run unless ``--apply``. Each selected thread is dismissed with its own
``DELETE /notifications/threads/<id>`` — GitHub's "mark as done"; marking read
would leave it in the inbox — and never with a mark-all.

The thread list is read through ``gh api`` on the repo-scoped notifications
path with ``all=true`` (the default lists unread threads only), paged up to
``MAX_PAGES``. Settled mode resolves PR states from one ``gh pr list --state
all`` read, then up to ``MAX_LOOKUPS`` one-by-one reads for PRs outside it; a
PR still unresolved is reported and left, never guessed settled.

Prints JSON ``{mode, dry_run, threads, selected, prs, dismissed, failed: [id],
unresolved: [pr], notice, summary}``: ``threads`` counts the PR threads
listed, ``selected`` those chosen (``--full`` lists each as ``{id, pr,
state}`` instead, with no ``prs`` or ``summary``), ``dismissed`` the deletes
that succeeded (0 in a dry run), ``notice`` a list or lookup failure (``None``
when clean) and ``summary`` the one report line. Exit 0 unless the thread list
itself was unreadable (1).

Stdlib only; a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling"). Tests
live in ``tests/test_pr_notifications.py``.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from typing import Callable

REPO = "DASMAC-com/dropset"
TIMEOUT = 60
PER_PAGE = 50
# 500 threads. Dismissed threads leave the list, so a backlog past this drains
# over successive runs rather than in one long one.
MAX_PAGES = 10
PR_LIST_LIMIT = 200
MAX_LOOKUPS = 20
SETTLED_STATES = ("MERGED", "CLOSED")
PR_URL_RE = re.compile(rf"/repos/{REPO}/pulls/(\d+)$")

# (gh args) -> (rc, stdout, stderr); `gh` itself is implied.
Gh = Callable[[list[str]], "tuple[int, str, str]"]


def real_gh(args: list[str]) -> tuple[int, str, str]:
    """``(rc, stdout, stderr)``; a timeout or a missing binary is an rc."""
    try:
        proc = subprocess.run(
            ["gh", *args], capture_output=True, text=True, timeout=TIMEOUT, check=False
        )
    except subprocess.TimeoutExpired:
        return 124, "", f"timed out after {TIMEOUT}s"
    except OSError as e:
        return 127, "", str(e)
    return proc.returncode, proc.stdout, proc.stderr


def first_line(text: str) -> str:
    return (text.strip().splitlines() or [""])[0][:160]


def _json(gh: Gh, args: list[str]):
    rc, out, err = gh(args)
    if rc != 0:
        raise RuntimeError(first_line(err) or f"gh exited {rc}")
    return json.loads(out or "null")


def list_threads(gh: Gh) -> list[tuple[str, int]]:
    """``[(thread id, PR number)]`` for every PR thread on the repo, read or
    not. Raises when a page is unreadable."""
    threads: list[tuple[str, int]] = []
    for page in range(1, MAX_PAGES + 1):
        path = f"/repos/{REPO}/notifications?all=true&per_page={PER_PAGE}&page={page}"
        raw = _json(gh, ["api", path]) or []
        for n in raw:
            m = PR_URL_RE.search(((n.get("subject") or {}).get("url")) or "")
            if m:
                threads.append((str(n["id"]), int(m.group(1))))
        if len(raw) < PER_PAGE:
            break
    return threads


def pr_states(gh: Gh, numbers: set[int]) -> tuple[dict[int, str], str | None]:
    """``({number: state}, notice)`` for ``numbers``, as far as resolvable."""
    states: dict[int, str] = {}
    notice = None
    try:
        listed = _json(
            gh,
            [
                "pr", "list", "--repo", REPO, "--state", "all",
                "--json", "number,state", "--limit", str(PR_LIST_LIMIT),
            ],
        )  # fmt: skip
        for pr in listed or []:
            states[int(pr["number"])] = pr.get("state")
    except (RuntimeError, ValueError, KeyError, TypeError) as e:
        notice = f"PR list failed ({e})"
    for num in sorted(numbers - set(states))[:MAX_LOOKUPS]:
        try:
            pr = _json(gh, ["pr", "view", str(num), "--repo", REPO, "--json", "state"])
        except (RuntimeError, ValueError):
            continue
        if isinstance(pr, dict) and pr.get("state"):
            states[num] = pr["state"]
    return {n: s for n, s in states.items() if n in numbers}, notice


def select(
    threads: list[tuple[str, int]],
    states: dict[int, str],
    prs: set[int] | None,
) -> tuple[list[dict], list[int]]:
    """``(selected, unresolved)``. ``prs`` given: every thread on those PRs.
    ``prs`` None: settled mode — threads whose PR state is merged or closed;
    a PR with no known state is unresolved, never selected."""
    selected: list[dict] = []
    unresolved: set[int] = set()
    for tid, num in threads:
        state = states.get(num)
        if prs is not None:
            if num in prs:
                selected.append({"id": tid, "pr": num, "state": state})
        elif state in SETTLED_STATES:
            selected.append({"id": tid, "pr": num, "state": state})
        elif state is None:
            unresolved.add(num)
    return selected, sorted(unresolved)


def dismiss(gh: Gh, prs: set[int] | None, apply: bool) -> tuple[int, dict]:
    """The whole act; ``prs`` None is settled mode."""
    result: dict = {
        "mode": "settled" if prs is None else "pr",
        "dry_run": not apply,
        "threads": 0,
        "selected": [],
        "dismissed": 0,
        "failed": [],
        "unresolved": [],
        "notice": None,
    }
    try:
        threads = list_threads(gh)
    except (RuntimeError, ValueError, KeyError, TypeError) as e:
        result["notice"] = f"notification list failed ({e})"
        return 1, result
    result["threads"] = len(threads)
    states: dict[int, str] = {}
    if prs is None:
        states, result["notice"] = pr_states(gh, {num for _, num in threads})
    result["selected"], result["unresolved"] = select(threads, states, prs)
    if not apply:
        return 0, result
    for t in result["selected"]:
        rc, _, _ = gh(["api", "-X", "DELETE", f"/notifications/threads/{t['id']}"])
        if rc == 0:
            result["dismissed"] += 1
        else:
            result["failed"].append(t["id"])
    return 0, result


def summary(result: dict) -> str:
    """One report line, for a caller that prints rather than parses."""
    n = len(result["selected"])
    if result["dry_run"]:
        line = f"would dismiss {n} notification(s)"
    else:
        line = f"dismissed {result['dismissed']} notification(s)"
        if result["failed"]:
            line += f", {len(result['failed'])} failed"
    if result["unresolved"]:
        line += f"; {len(result['unresolved'])} PR(s) unresolved, left"
    if result["notice"]:
        line += f"; {result['notice']}"
    return line


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="pr_notifications.py")
    sub = parser.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("dismiss", help="dismiss PR notification threads")
    which = p.add_mutually_exclusive_group(required=True)
    which.add_argument(
        "--pr", type=int, action="append", help="a PR whose threads to dismiss"
    )
    which.add_argument(
        "--settled",
        action="store_true",
        help="every thread whose PR is merged or closed",
    )
    p.add_argument("--apply", action="store_true", help="dismiss for real")
    p.add_argument(
        "--full", action="store_true", help="print every selected thread, not counts"
    )
    return parser.parse_args(argv[1:])


def compact(result: dict) -> dict:
    """The default print: the selected threads as a count plus their PRs,
    since a first settled run selects a hundred-odd threads."""
    out = {k: v for k, v in result.items() if k != "selected"}
    out["selected"] = len(result["selected"])
    out["prs"] = sorted({t["pr"] for t in result["selected"]})
    out["summary"] = summary(result)
    return out


def main() -> int:
    args = parse_args(sys.argv)
    rc, result = dismiss(real_gh, None if args.settled else set(args.pr), args.apply)
    json.dump(result if args.full else compact(result), sys.stdout, indent=2)
    sys.stdout.write("\n")
    return rc


if __name__ == "__main__":
    sys.exit(main())
