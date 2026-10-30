#!/usr/bin/env python3
# cspell:word ppid
"""Atomic session close — write the issue's closing state, then remove the
worktree, as one act a session runs as its **last** tool call on the operator's
explicit ratification in its own tab.

The ordering is the whole point (operator ruling, 2026-10-12): the state write
lands **first** and the worktree goes **second**, never the reverse. A worktree
removed before its issue leaves In Progress / In Review strands the session — a
later resume lands in a different checkout, or cannot resume at all — and the
issue still reads as live work. Before this tool every close was a two-step the
operator finished by hand, in whichever order they happened to reach for.

Usage (run from inside the worktree being closed):

    python3 .claude/tools/close_session.py --state Done --leftovers-file L.json
    python3 .claude/tools/close_session.py --state Done --leftovers-file L.json \
        --issue ENG-1571
    python3 .claude/tools/close_session.py --state 'In Review' --keep-worktree
    python3 .claude/tools/close_session.py --state Done --leftovers-file L.json \
        --dry-run
    python3 .claude/tools/close_session.py --park --session-id <uuid>

``--state`` is required (unless ``--park``) and never guessed: an
implementation session closes to Done, an ``architect`` / ``explore`` session hands off at In Review. The issue
is read from an ``eng-###`` or ``worktree-###`` branch; any other branch (a
pre-rename ``worktree-ceo-…``) must pass ``--issue``. ``--keep-worktree`` is the report-only ending — the same gates
and state write, with the removal left to the operator. It is **required** for
any state other than Done / Canceled: removing a worktree whose issue still
reads as live is the stranding this tool exists to prevent, and ``upkeep``
would never collect it.

``--park`` ends a session whose reviewed PR is **held** — behind another PR or
a CI break (``review-pr``'s park, ``plan``'s hold message) — so its fleet slot
frees while the PR stays open. It takes no ``--state`` and writes none (the
issue keeps the state it has), keeps the worktree and branch for the resume,
dismisses no notification threads (the PR is live), and appends the cost line
when ``--session-id`` is given. Its gates replace the open-PR one: the tree is
clean and pushed (the same check as below), the branch has an open PR, and the
issue body carries a resume brief — the paragraph holding the last
``resume brief`` mention names that PR after it (``#N``, ``PR N`` or a
``/pull/N`` link), so a brief left by an earlier park of another PR does not
count. Each refuses with the missing precondition named. The open-PR refusal
is unchanged for every other mode, Done and Canceled included.

A park then **ends its own client**, so the slot frees with no operator hand.
The client is the pid in the harness's worktree lock, proven an ancestor as
for a removal **and** named ``claude`` (an ancestor alone could be the shell
or the terminal above it); a missing, foreign or misnamed lock refuses before
anything is written. It gets SIGTERM with a bounded wait as the tool's
**last** act, re-proven an ancestor immediately before (a client that died
meanwhile frees its pid for reuse). The kill follows the cost line because a
kill any earlier could cut the session off before its durable record is
written; the printed result is best-effort, since the killed client seldom
reads it, so a failed cost-line append exits 1 and keeps the client alive to
see why. A resume works from the transcript and the brief, so the kill is
reversible. ``--no-exit`` is the human-driven park that keeps the tab.

Every gate runs **before** any write, and each fails closed:

* the tree is clean and every commit is pushed — the pruner's own
  :func:`prune_worktrees.unsafe_reason`, except that a branch with no upstream
  passes when it carries nothing beyond ``origin/main`` (an architect worktree
  never pushes) or when HEAD is the recorded head of its merged PR (a
  squash-merged branch whose remote was deleted on merge and pruned);
* no PR on the branch is open (``gh pr list``; an unreadable list refuses);
* the worktree's lock, when removing, is **this** session's: the harness locks
  a live session's worktree as ``claude session <name> (pid N …)``, and the
  pid must be an ancestor of this process. Another live session's lock is never
  lifted.

Then the state is written through ``board_batch`` (zero-echo GraphQL); only on
a confirmed write are the branch's PR notification threads dismissed
(``pr_notifications``, best-effort — a failure is a note) and the worktree
unlocked, removed **without** ``--force``, and its branch deleted. A removal that fails after the write re-locks the tree and
exits 2. The issue is resolved by then (a removal follows only a Done or
Canceled write), so ``upkeep`` passes the tree to the pruner as a closed name:
no upstream and unpushed work no longer hold it, and the restored lock goes
stale once this session exits, so the next pass collects it. The exit-2 note
says so, and how to remove it by hand sooner.

``--leftovers-file <path>`` is the **leftover ledger**, and a Done or Canceled
close requires it: a JSON array with one ``{"item", "disposition", "ref"}``
object per warning, nit or "left for human review" item the session surfaced
and did not do — ``[]`` when there were none. Each disposition is ``filed``
(``ref`` the follow-up's ``ENG-###``), ``handed`` (``ref`` the live session it
was messaged to, never the operator) or ``dropped`` (``ref`` the reason). Anything else refuses
before any write — "noted in the PR body" above all, which is a record, not a
disposition: once the worktree goes the session has no shell left to file
with. The gate is mechanical so the close question can run it first, as a
``--dry-run``, and ask only once it passes. An In Review handoff carries its
leftovers on the live issue and needs no ledger.

``--session-id <uuid>`` adds the session's one-line cost estimate
(``session_metrics.cost_line``) to the issue body, appended zero-echo once the
state write is confirmed and before the removal. It is priced before the write,
so a session that cannot be priced is known before anything changes. A session id that
resolves to no transcript skips the line and says so in ``notes``; a failed
append is a note too, never a refusal, since the state has landed. In the
result, ``cost_line`` is the line appended — or, on a dry run, the line that
would be. A re-run after an exit 2 appends a second line; remove the tree by
hand instead.

Prints one JSON object ``{issue, state, worktree, branch, pr, dry_run,
keep_worktree, park, refused, leftovers, state_written, notifications, removed,
cost_line, client_pid, notes}`` (``client_pid`` the client a park ends, or
on a dry run would; ``state`` is ``None`` and ``keep_worktree`` true on a park;
``leftovers`` the ledger's item count, ``None`` when none
was read; ``notifications`` the dismissal's one-line summary, ``None`` when no
PR or no write). Exit 0 on a complete
close; 1 on a refusal or error before any removal — a gate refusal changes
nothing, while a write reported "unconfirmed" may have landed; 2 when the state
landed but the removal did not.

The harness's ``ExitWorktree`` is deliberately not the mechanism: it documents
itself as operating only on worktrees made by ``EnterWorktree`` in the same
session, and whether a ``claude --worktree`` launch counts is unverified — the
only non-destructive probe would itself move the session out of its checkout.

Stdlib only; a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Callable

import board_batch
import linear_issue
import pr_notifications
import prune_worktrees
import session_metrics

REPO = "DASMAC-com/dropset"
TIMEOUT = 60
# `eng-<n>` once `init-pr` renamed it, or the `worktree-<n>` a read-only
# architect / explore session never renames — the same set `upkeep.BRANCH_RE`
# ties to an issue.
ISSUE_BRANCH_RE = re.compile(
    r"^(?:worktree-)?(?:eng-)?(\d+)$", re.IGNORECASE | re.ASCII
)
ISSUE_RE = re.compile(r"^ENG-\d+$")
LOCK_PID_RE = re.compile(r"\(pid (\d+)\b")
BRIEF_RE = re.compile(r"resume brief", re.IGNORECASE)
# The park's bounded wait for its client to exit after SIGTERM, in seconds.
EXIT_WAIT = 10.0
EXIT_POLL = 0.25
# The harness client's command name, as `ps -o comm=` reports it (basename).
CLIENT_COMM = "claude"
NO_UPSTREAM_PREFIX = "no upstream"
MAX_ANCESTRY = 64
# Matched by name, lowercased: an unrecognized name fails closed into
# requiring --keep-worktree.
REMOVABLE_STATES = ("done", "canceled", "cancelled")
# A leftover's disposition, and what its `ref` must be: a filed follow-up's
# number, the session it was handed to, or the reason it was dropped.
DISPOSITIONS = ("filed", "handed", "dropped")
# Handing a leftover to the operator is the ending the ledger exists to stop.
NOT_A_SESSION = ("operator", "human", "user", "you")

Runner = Callable[[list[str], "str | None"], "tuple[int, str, str]"]


class Refusal(Exception):
    """A gate failed; nothing has been written or removed."""


def real_run(cmd: list[str], cwd: str | None = None) -> tuple[int, str, str]:
    """``(rc, stdout, stderr)``; a timeout or a missing binary is an rc."""
    try:
        proc = subprocess.run(
            cmd, capture_output=True, text=True, timeout=TIMEOUT, cwd=cwd, check=False
        )
    except subprocess.TimeoutExpired:
        return 124, "", f"timed out after {TIMEOUT}s"
    except OSError as e:
        return 127, "", str(e)
    return proc.returncode, proc.stdout, proc.stderr


def first_line(text: str) -> str:
    return (text.strip().splitlines() or [""])[0][:200]


# --- pure helpers ----------------------------------------------------------


def issue_for(branch: str, explicit: str | None) -> str:
    """The ``ENG-###`` to close: the branch name, or ``--issue`` where the
    branch names none. Both present and disagreeing is a refusal."""
    m = ISSUE_BRANCH_RE.match(branch)
    from_branch = f"ENG-{int(m.group(1))}" if m else None
    if explicit:
        ident = explicit.strip().upper()
        if not ISSUE_RE.match(ident):
            raise Refusal(f"--issue must look like ENG-###, got {explicit!r}")
        if from_branch and ident != from_branch:
            raise Refusal(f"--issue {ident} disagrees with branch {branch!r}")
        return ident
    if not from_branch:
        raise Refusal(f"branch {branch!r} names no eng-### issue — pass --issue")
    return from_branch


def lock_reasons(porcelain: str) -> dict[str, str | None]:
    """``{worktree path: lock reason}`` from ``git worktree list --porcelain``;
    an unlocked tree maps to ``None``, a reasonless lock to ``""``."""
    out: dict[str, str | None] = {}
    current: str | None = None
    for line in porcelain.splitlines():
        if line.startswith("worktree "):
            # Resolved, like the cwd it is looked up by: a symlinked path
            # (/tmp, /var) must not read as unlocked.
            current = str(Path(line[len("worktree ") :].strip()).resolve())
            out[current] = None
        elif current is not None and (line == "locked" or line.startswith("locked ")):
            out[current] = line[len("locked") :].strip()
    return out


def pid_alive(pid: int) -> bool:
    """Whether ``pid`` names a running process (a permission error counts)."""
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def lock_verdict(
    reason: str | None, ancestors: set[int], alive: Callable[[int], bool] = pid_alive
) -> bool:
    """Whether removal must unlock first. Raises :class:`Refusal` on a lock this
    process cannot prove is its own session's — a stale lock included, which
    is reported as stale rather than lifted."""
    if reason is None:
        return False
    m = LOCK_PID_RE.search(reason)
    if not m:
        raise Refusal(f"worktree is locked with no session pid ({reason!r})")
    pid = int(m.group(1))
    if pid not in ancestors:
        if alive(pid):
            raise Refusal(
                f"worktree is locked by another live process (pid {pid}), "
                "not this session"
            )
        raise Refusal(
            f"worktree carries a stale lock from pid {pid}, which is not "
            "running; unlock it by hand if no session owns this tree"
        )
    return True


def leftover_gaps(ledger: object) -> list[str]:
    """Why each leftover in a parsed ledger lacks a disposition; ``[]`` when
    every item has one. A ledger that is not a list is one gap."""
    if not isinstance(ledger, list):
        return ["the ledger is not a JSON array"]
    gaps: list[str] = []
    for n, entry in enumerate(ledger, 1):
        if not isinstance(entry, dict):
            gaps.append(f"#{n} is not an object")
            continue
        item = str(entry.get("item") or "").strip()
        label = f"#{n} ({item[:60]!r})" if item else f"#{n}"
        raw = str(entry.get("disposition") or "").strip()
        disposition = raw.lower()
        ref = str(entry.get("ref") or "").strip()
        if not item:
            gaps.append(f"{label} names no item")
        elif disposition not in DISPOSITIONS:
            gaps.append(
                f"{label} has disposition {raw or 'none'!r}; "
                f"want one of {', '.join(DISPOSITIONS)}"
            )
        elif disposition == "filed" and not ISSUE_RE.match(ref.upper()):
            gaps.append(f"{label} is filed but its ref {ref!r} is no ENG-###")
        elif not ref:
            gaps.append(f"{label} is {disposition} with no ref")
        elif disposition == "handed" and ref.lower() in NOT_A_SESSION:
            gaps.append(f"{label} is handed to {ref!r}, not a live session")
    return gaps


def read_leftovers(path: str | None, state: str) -> int | None:
    """The ledger's item count, after its gate. Raises :class:`Refusal` on a
    removable state with no ledger, an unreadable one, or any item without a
    disposition; a live state reads no ledger and returns ``None``."""
    if path is None:
        if state.strip().lower() in REMOVABLE_STATES:
            raise Refusal(
                f"a {state} close needs --leftovers-file (a JSON array, [] when "
                "nothing was left over): every leftover filed, handed or "
                "dropped before the close"
            )
        return None
    try:
        ledger = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
        raise Refusal(f"unreadable --leftovers-file {path}: {e}") from e
    gaps = leftover_gaps(ledger)
    if gaps:
        raise Refusal(
            f"{len(gaps)} leftover(s) have no disposition — file, hand off or "
            f"drop each before the close: {'; '.join(gaps)}"
        )
    return len(ledger)


def pr_verdict(prs: list[dict]) -> dict | None:
    """The PR to report (an open one beats the rest); raises on an open one."""
    open_prs = [p for p in prs if p.get("state") == "OPEN"]
    if open_prs:
        raise Refusal(f"PR #{open_prs[0].get('number')} is still open")
    return prs[0] if prs else None


def park_pr(prs: list[dict]) -> dict:
    """The open PR a park holds; raises when there is none to hold."""
    open_prs = [p for p in prs if p.get("state") == "OPEN"]
    if not open_prs:
        raise Refusal("--park needs an open PR on the branch; none is open")
    return open_prs[0]


def brief_verdict(body: str, number: int) -> None:
    """Raises unless the paragraph holding the last ``resume brief`` mention in
    ``body`` names PR ``number`` after it — as ``#N``, ``PR N`` or a
    ``/pull/N`` link, so a stored autolink still reads. Bounded at the next
    blank line, so a later checklist naming the PR cannot vouch for a stale
    brief."""
    marks = list(BRIEF_RE.finditer(body))
    if not marks:
        raise Refusal(
            "--park needs a resume brief on the issue (linear_issue.py append "
            "a '**Resume brief**: PR #N …' naming the PR, the PR waited on, "
            "gates owed)"
        )
    start = marks[-1].start()
    end = body.find("\n\n", start)
    brief = body[start : end if end != -1 else len(body)]
    if not re.search(rf"(?:#|\bPR |/pull/){number}\b", brief):
        raise Refusal(f"the issue's resume brief does not name PR #{number}")


# --- probes ----------------------------------------------------------------


def ancestor_pids(run: Runner, start: int) -> set[int]:
    """This process and every parent up to init, read through ``ps``."""
    seen: set[int] = set()
    pid = start
    while pid > 1 and pid not in seen and len(seen) < MAX_ANCESTRY:
        seen.add(pid)
        rc, out, _ = run(["ps", "-o", "ppid=", "-p", str(pid)], None)
        text = out.strip()
        if rc != 0 or not text.isdigit():
            break
        pid = int(text)
    return seen


def git_safety(run: Runner, path: str, branch: str, prs: list[dict]) -> None:
    """The pruner's uncommitted/unpushed gate, plus the two no-upstream
    carve-outs: nothing to push, or HEAD already recorded as a merged PR's
    head (its remote branch deleted on merge, then pruned by a fetch)."""

    def git(args: list[str]) -> tuple[int, str, str]:
        return run(["git", *args], None)

    reason = prune_worktrees.unsafe_reason(path, branch, git)
    if reason is not None and reason.startswith(NO_UPSTREAM_PREFIX):
        rc, head, _ = git(["-C", path, "rev-parse", "HEAD"])
        merged_heads = {p.get("headRefOid") for p in prs if p.get("state") == "MERGED"}
        if rc == 0 and head.strip() and head.strip() in merged_heads:
            return
        rc, out, _ = git(["-C", path, "rev-list", "--count", "origin/main..HEAD"])
        count = out.strip()
        if rc == 0 and count == "0":
            return
        reason = (
            f"no upstream and {count} commit(s) beyond origin/main"
            if rc == 0 and count.isdigit()
            else reason
        )
    if reason is not None:
        raise Refusal(reason)


def read_prs(run: Runner, branch: str) -> list[dict]:
    rc, out, err = run(
        [
            "gh", "pr", "list", "--repo", REPO, "--head", branch,
            "--state", "all", "--json", "number,state,headRefOid",
        ],
        None,
    )  # fmt: skip
    if rc != 0:
        raise Refusal(f"could not read the branch's PRs: {first_line(err)}")
    try:
        prs = json.loads(out or "[]")
    except json.JSONDecodeError as e:
        raise Refusal(f"unreadable gh pr list output: {e}") from e
    if not isinstance(prs, list):
        raise Refusal("unreadable gh pr list output")
    return prs


def locate(run: Runner) -> tuple[str, str, str, str]:
    """``(worktree, branch, base, worktree-list porcelain)`` for the cwd."""
    rc, top, err = run(["git", "rev-parse", "--show-toplevel"], None)
    if rc != 0:
        raise Refusal(f"not inside a git worktree: {first_line(err)}")
    rc, porcelain, err = run(["git", "worktree", "list", "--porcelain"], None)
    if rc != 0:
        raise Refusal(f"git worktree list failed: {first_line(err)}")
    trees = prune_worktrees.parse_worktrees(porcelain)
    bases = [t for t in trees if prune_worktrees.is_base(t)]
    if not bases:
        raise Refusal("main is not checked out in any worktree")
    worktree = str(Path(top.strip()).resolve())
    base = str(Path(bases[0]["path"]).resolve())
    if worktree == base:
        raise Refusal("run from the session's worktree, not the base checkout")
    mine = [t for t in trees if str(Path(t["path"]).resolve()) == worktree]
    if not mine or not mine[0].get("branch"):
        raise Refusal("this worktree has no branch checked out")
    return worktree, mine[0]["branch"], base, porcelain


# --- the act ---------------------------------------------------------------


def write_state(issue: str, state: str, dry_run: bool) -> None:
    """One state write through ``board_batch``; a dry run resolves the state
    name without writing. Raises on any failure."""
    api_key = board_batch.env_var("LINEAR_API_KEY")
    project = board_batch.env_var("LINEAR_PROJECT_ID")
    updates = {issue: {"state": state}}
    by_number = board_batch.index_by_number(
        board_batch.fetch_issues_by_number(
            api_key, project, board_batch.referenced_numbers(updates)
        )
    )
    board_batch.apply_fields(
        api_key, updates, by_number, dry_run=dry_run, emit=lambda _line: None
    )


def dismiss_threads(run: Runner, prs: list[dict], result: dict) -> None:
    """Dismiss the branch's PR threads, before the removal. Best-effort: the
    state already landed and the gates passed, so a failure is a note — and
    upkeep's settled run collects whatever is left the next morning."""
    numbers = {int(p["number"]) for p in prs if p.get("number") is not None}
    if not numbers:
        return
    # Unsubscribe only on merge: a closed PR can be reopened.
    keep = {
        int(p["number"])
        for p in prs
        if p.get("number") is not None and p.get("state") != "MERGED"
    }
    try:
        rc, out = pr_notifications.dismiss(
            lambda gh_args: run(["gh", *gh_args], None),
            numbers,
            apply=True,
            keep_subscription=keep,
        )
    except Exception as e:  # noqa: BLE001 — the state landed; never lose the removal
        result["notifications"] = f"dismissal failed: {type(e).__name__}: {e}"
        result["notes"].append(f"notifications: {result['notifications']}")
        return
    result["notifications"] = pr_notifications.summary(out)
    if rc != 0 or out["failed"] or out["unsubscribe_failed"] or out["notice"]:
        result["notes"].append(f"notifications: {result['notifications']}")


def remove(
    run: Runner,
    base: str,
    worktree: str,
    branch: str,
    lock: str | None,
    notes: list[str],
):
    """Unlock (when ours), remove without ``--force``, delete the branch.
    Returns ``None`` once the tree is gone, else the reason it is still there;
    a failed branch delete or prune after that is appended to ``notes``."""

    def git(*args: str) -> tuple[int, str, str]:
        return run(["git", "-C", base, *args], None)

    if lock is not None:
        rc, _, err = git("worktree", "unlock", worktree)
        if rc != 0:
            return f"unlock failed: {first_line(err)}"
    rc, _, err = git("worktree", "remove", worktree)
    if rc != 0:
        why = f"worktree remove refused: {first_line(err)}"
        if lock is not None:
            # Restore the session's protection; the tree is still live.
            l_rc, _, l_err = git("worktree", "lock", "--reason", lock, worktree)
            if l_rc != 0:
                why += f"; re-lock FAILED, tree left unlocked: {first_line(l_err)}"
        return why
    # A squash-merged tip is not an ancestor of main, so -d would refuse; the
    # gates above proved every commit pushed.
    for args in (("branch", "-D", branch), ("worktree", "prune")):
        a_rc, _, a_err = git(*args)
        if a_rc != 0:
            notes.append(f"git {' '.join(args[:2])} failed: {first_line(a_err)}")
    return None


def append_line(issue: str, line: str) -> None:
    """The cost line, through the zero-echo body append. Raises on failure."""
    linear_issue.append_body(
        board_batch.env_var("LINEAR_API_KEY"), issue, line, dry_run=False
    )


def read_body(issue: str) -> str:
    """The issue body, fetched zero-echo, for the park's brief gate."""
    found = linear_issue.fetch_issue_body(board_batch.env_var("LINEAR_API_KEY"), issue)
    return found.get("description") or ""


def price_session(session_id: str | None, notes: list[str]) -> str | None:
    """The session's cost line, or ``None`` with a note saying why it is
    skipped. Priced before any write, so the close never waits on a
    transcript read between the state landing and the removal."""
    if not session_id:
        return None
    try:
        return session_metrics.session_cost_line(session_id)
    except Exception as e:  # noqa: BLE001 — a skipped line never stops a close
        notes.append(f"cost line skipped, session {session_id} not priced: {e}")
        return None


def parked(
    args: argparse.Namespace,
    result: dict,
    coster: Callable[[str | None, list[str]], str | None],
    appender: Callable[[str, str], None],
) -> tuple[int, dict]:
    """The park's act, once its gates passed: the cost line and nothing else —
    no state, no removal, no dismissal of a live PR's threads."""
    line = coster(args.session_id, result["notes"])
    if args.dry_run:
        result["cost_line"] = line
        result["notes"].append("dry run: park gates passed")
        return 0, result
    if line is not None:
        try:
            appender(result["issue"], line)
            result["cost_line"] = line
        except Exception as e:  # noqa: BLE001 — nothing to undo; keep the client
            # A killed client never reads this result, so a failed append
            # must keep it alive to see the note: exit 1, and no kill.
            result["notes"].append(
                f"cost line not appended: {e}; client left running so this is "
                "seen — append the line by hand, then re-run --park"
            )
            return 1, result
    result["notes"].append(
        f"parked: PR #{result['pr'].get('number')} open, state unchanged, "
        "worktree and branch kept for the resume"
    )
    if result["client_pid"] is not None:
        result["notes"].append(
            f"ending the client (pid {result['client_pid']}) once this prints; "
            "resume from the brief in a fresh session"
        )
    return 0, result


def command_name(run: Runner, pid: int) -> str:
    """The basename of ``pid``'s command, through ``ps``; ``""`` if unread."""
    rc, out, _ = run(["ps", "-o", "comm=", "-p", str(pid)], None)
    return Path(out.strip()).name if rc == 0 else ""


def client_pid(
    reason: str | None,
    ancestors: set[int],
    alive: Callable[[int], bool],
    run: Runner,
) -> int:
    """The session's own client, read from the harness's worktree lock,
    proven an ancestor by :func:`lock_verdict` and named ``CLIENT_COMM`` — an
    ancestor alone could be a shell or the terminal above the client. Raises
    when it cannot be."""
    if reason is None:
        raise Refusal(
            "worktree carries no session lock, so the client to end is "
            "unknown — pass --no-exit and close the tab by hand"
        )
    lock_verdict(reason, ancestors, alive)
    m = LOCK_PID_RE.search(reason)
    assert m is not None  # lock_verdict refused a pid-less lock
    pid = int(m.group(1))
    name = command_name(run, pid)
    if name != CLIENT_COMM:
        raise Refusal(
            f"the lock's pid {pid} runs {name or 'nothing readable'!r}, not "
            f"{CLIENT_COMM!r}, so it is not ended — pass --no-exit"
        )
    return pid


def still_ours(pid: int, run: Runner = real_run, start: int | None = None) -> bool:
    """Re-proves, just before the kill, that ``pid`` is still an ancestor named
    ``CLIENT_COMM``: a client that died after the gates leaves its pid free for
    reuse, and the cost-line write between the two can take seconds."""
    ancestors = ancestor_pids(run, os.getpid() if start is None else start)
    return pid in ancestors and command_name(run, pid) == CLIENT_COMM


def end_client(
    pid: int,
    kill: Callable[[int, int], None] = os.kill,
    alive: Callable[[int], bool] = pid_alive,
    sleep: Callable[[float], None] = time.sleep,
) -> bool:
    """SIGTERM the client, then wait up to ``EXIT_WAIT`` seconds for it to go.
    Whether it went; this process is its descendant, so a ``True`` is seldom
    seen."""
    try:
        kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        return True
    for _ in range(int(EXIT_WAIT / EXIT_POLL)):
        if not alive(pid):
            return True
        sleep(EXIT_POLL)
    return not alive(pid)


def close(
    args: argparse.Namespace,
    run: Runner = real_run,
    writer: Callable[[str, str, bool], None] = write_state,
    pid: int | None = None,
    alive: Callable[[int], bool] = pid_alive,
    coster: Callable[[str | None, list[str]], str | None] = price_session,
    appender: Callable[[str, str], None] = append_line,
    body_reader: Callable[[str], str] = read_body,
) -> tuple[int, dict]:
    park = getattr(args, "park", False)
    result: dict = {
        "issue": None,
        "state": args.state,
        "worktree": None,
        "branch": None,
        "pr": None,
        "dry_run": args.dry_run,
        "keep_worktree": args.keep_worktree or park,
        "park": park,
        "refused": None,
        "leftovers": None,
        "state_written": False,
        "notifications": None,
        "removed": False,
        "cost_line": None,
        "client_pid": None,
        "notes": [],
    }
    try:
        worktree, branch, base, porcelain = locate(run)
        result.update(worktree=worktree, branch=branch)
        result["issue"] = issue_for(branch, args.issue)
        if park:
            if args.state is not None:
                raise Refusal("--park writes no state; drop --state")
        elif getattr(args, "no_exit", False):
            raise Refusal("--no-exit applies only to --park")
        if park:
            prs = read_prs(run, branch)
            git_safety(run, worktree, branch, prs)
            result["pr"] = park_pr(prs)
            try:
                body = body_reader(result["issue"])
            except Exception as e:  # noqa: BLE001 — an unread brief is no brief
                raise Refusal(f"could not read the issue body: {e}") from e
            brief_verdict(body, int(result["pr"]["number"]))
            if not getattr(args, "no_exit", False):
                result["client_pid"] = client_pid(
                    lock_reasons(porcelain).get(worktree),
                    ancestor_pids(run, os.getpid() if pid is None else pid),
                    alive,
                    run,
                )
            return parked(args, result, coster, appender)
        if args.state is None:
            raise Refusal("--state is required unless --park")
        if (
            not args.keep_worktree
            and args.state.strip().lower() not in REMOVABLE_STATES
        ):
            raise Refusal(
                f"state {args.state!r} leaves the issue live, so the worktree "
                "must stay — pass --keep-worktree"
            )
        result["leftovers"] = read_leftovers(args.leftovers_file, args.state)
        prs = read_prs(run, branch)
        git_safety(run, worktree, branch, prs)
        result["pr"] = pr_verdict(prs)
        lock = None
        if not args.keep_worktree:
            reason = lock_reasons(porcelain).get(worktree)
            ancestors = ancestor_pids(run, os.getpid() if pid is None else pid)
            if lock_verdict(reason, ancestors, alive):
                lock = reason
    except Refusal as e:
        result["refused"] = str(e)
        return 1, result

    line = coster(args.session_id, result["notes"])
    try:
        writer(result["issue"], args.state, args.dry_run)
    except Exception as e:  # noqa: BLE001 — any failure must stop the removal
        result["refused"] = f"state write failed or unconfirmed, nothing removed: {e}"
        return 1, result
    if args.dry_run:
        result["cost_line"] = line
        result["notes"].append("dry run: gates passed, state name resolved")
        return 0, result
    result["state_written"] = True
    if line is not None:
        try:
            appender(result["issue"], line)
            result["cost_line"] = line
        except Exception as e:  # noqa: BLE001 — the state landed; go on
            result["notes"].append(f"cost line not appended: {e}")
    dismiss_threads(run, prs, result)

    if args.keep_worktree:
        result["notes"].append("worktree kept; the operator removes it")
        return 0, result
    why = remove(run, base, worktree, branch, lock, result["notes"])
    if why is not None:
        result["notes"].append(
            f"{why}; upkeep's next pass collects it once this session exits "
            "(the issue is closed, so only a live lock holds it), or remove it "
            f"by hand from the base checkout (git worktree remove {worktree})"
        )
        return 2, result
    result["removed"] = True
    return 0, result


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="close_session.py")
    parser.add_argument(
        "--state", default=None, help="closing state name, e.g. Done or 'In Review'"
    )
    parser.add_argument(
        "--park",
        action="store_true",
        help="end a held session: PR open, no state write, worktree kept",
    )
    parser.add_argument(
        "--no-exit",
        action="store_true",
        help="with --park: leave the client running (a human-driven park)",
    )
    parser.add_argument("--issue", default=None, help="ENG-### when the branch is not")
    parser.add_argument(
        "--keep-worktree",
        action="store_true",
        help="write the state but leave the worktree for the operator",
    )
    parser.add_argument(
        "--leftovers-file",
        default=None,
        help="JSON leftover ledger; required for a Done or Canceled close",
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="run every gate, write nothing"
    )
    parser.add_argument(
        "--session-id",
        default=None,
        help="the session UUID: append its cost line to the issue after the state",
    )
    return parser.parse_args(argv[1:])


def main() -> int:
    rc, result = close(parse_args(sys.argv))
    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    sys.stdout.flush()
    # Last, after every write: a kill any earlier could land before the cost
    # line, the session's durable record. The printed result is best-effort —
    # the killed client seldom reads it — which is why a failed append exits 1
    # above and never reaches here.
    pid = result["client_pid"]
    if rc == 0 and not result["dry_run"] and pid is not None:
        if not still_ours(pid):
            return _warn(f"client pid {pid} is no longer this session's; not ended")
        if not end_client(pid):
            return _warn(
                f"client pid {pid} still running after SIGTERM and {EXIT_WAIT}s; "
                "close the tab by hand"
            )
    return rc


def _warn(text: str) -> int:
    """One stderr line, exit 1; a pipe the dead client took is no error."""
    try:
        sys.stderr.write(text + "\n")
    except BrokenPipeError:
        pass
    return 1


if __name__ == "__main__":
    sys.exit(main())
