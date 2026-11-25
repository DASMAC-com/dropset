#!/usr/bin/env python3
"""PR notification dismissal — the one rule every caller applies, instead of
the two that drifted (upkeep dismissed only behind a Linear-closed gate, the
queue watcher on merged only, and the close paths never).

Usage:

    python3 .claude/tools/pr_notifications.py dismiss --pr 663 [--pr 664] \
      [--keep-subscription] [--apply]
    python3 .claude/tools/pr_notifications.py dismiss --pr 663 --hold-attention \
      [--apply]
    python3 .claude/tools/pr_notifications.py dismiss --settled [--apply]

``--pr`` selects every thread on the named PRs, whatever their state — the
close path's form, which has already refused an open PR. **``--hold-attention``**
(``--pr`` only) then leaves in the inbox every selected thread whose GitHub
``reason`` is in ``ATTENTION_REASONS`` — a mention, a review request, a reply
in a joined conversation — and lists each in ``held`` as ``{id, pr, state,
reason, title}``: the queue watcher holds them at the merge so the
``post-merge`` skill can answer each before its close, whose own ``--pr`` run
(no hold) clears them. ``--settled`` selects
every thread whose PR is merged or closed, whatever its issue's Linear state:
a merged PR whose issue sits In Review is settled code, and its threads are
the bulk of the inbox. An open PR's threads are never selected by it.

A dry run unless ``--apply``. **Unsubscribe only on merge, on the ``--pr``
path**: there a thread on a merged PR is first unsubscribed by ignoring it
(``PUT /notifications/threads/<id>/subscription`` with ``ignored=true``), so a
later bot comment on the merged PR cannot re-create it — a ``DELETE`` on the
subscription would not hold, since a watcher of the repository still receives
the thread's notifications. Every selected thread is then dismissed with its
own ``DELETE /notifications/threads/<id>`` — GitHub's "mark as done"; marking
read would leave it in the inbox — and never with a mark-all. A thread on a PR
not merged is marked done only, its subscription kept: an open PR's is what
re-creates the thread on new activity, and a closed PR can be reopened, so a
``--pr`` caller passes ``--keep-subscription`` (in process,
``keep_subscription``) for PRs not merged. **``--settled`` makes one call per
thread** — the ``DELETE`` alone, never the ignore: the merge tail already
unsubscribed each merged PR's thread at the merge, and ``init-pr`` muted it at
creation, so a second, sequential ``gh`` per thread only doubled the sweep.

The queue watcher (``wait_for_checks.py --queue``) calls :func:`dismiss` in
process on every exit but ``probe_failed``, holding attention threads and
keeping the subscription unless the verdict is ``merged``; the close path
dismisses every thread, held ones included, and keeps the subscription for any
PR not merged;
upkeep's ``notifications`` step and the scheduler's close-out run
``dismiss --settled --apply``.

The thread list is read through ``gh api`` on the repo-scoped notifications
path with ``all=true`` (the default lists unread threads only), paged up to
``MAX_PAGES``. Settled mode resolves PR states from one ``gh pr list --state
all`` read, then up to ``MAX_LOOKUPS`` one-by-one reads for PRs outside it; a
PR still unresolved is reported and left, never guessed settled.

Prints JSON ``{mode, dry_run, threads, selected, held, prs, dismissed, unsubscribed,
failed: [id], unsubscribe_failed: [id], error, unresolved: [pr], notice,
summary}``: ``threads`` counts the PR threads listed, ``selected`` those chosen
(``--full`` lists each as ``{id, pr, state}`` instead, with no ``prs`` or
``summary``), ``dismissed`` / ``unsubscribed`` the calls that succeeded (0 in a
dry run), ``failed`` / ``unsubscribe_failed`` the thread ids whose call failed,
``error`` the first failed call's reason, ``notice`` a list or
lookup failure (``None`` when clean) and ``summary`` the one report line. Exit 0 unless the thread list
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
# GitHub's thread reasons that mean someone wants an answer from this account:
# a mention (direct or of a team), a review request or assignment, and
# `comment` — a reply in a conversation this account already joined, which is
# how a review comment's follow-up arrives. Every other reason (`author`,
# `ci_activity`, `state_change`, `subscribed`, ...) is the bulk, safe to dismiss.
ATTENTION_REASONS = (
    "assign",
    "comment",
    "mention",
    "review_requested",
    "team_mention",
)


def pr_url_re(repo: str) -> re.Pattern:
    return re.compile(rf"/repos/{re.escape(repo)}/pulls/(\d+)$")


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


def list_threads(
    gh: Gh, repo: str = REPO, meta: dict[str, dict] | None = None
) -> list[tuple[str, int]]:
    """``[(thread id, PR number)]`` for every PR thread on the repo, read or
    not; ``meta``, when given, is filled with each thread's ``{reason,
    title}``. Raises when a page is unreadable."""
    url_re = pr_url_re(repo)
    threads: list[tuple[str, int]] = []
    for page in range(1, MAX_PAGES + 1):
        path = f"/repos/{repo}/notifications?all=true&per_page={PER_PAGE}&page={page}"
        raw = _json(gh, ["api", path])
        if raw is None:
            raw = []
        if not isinstance(raw, list):
            # An rc-0 object (an error body) is unreadable, never zero threads.
            raise ValueError(f"page {page} is not a thread list")
        for n in raw:
            subject = n.get("subject") if isinstance(n, dict) else None
            if not isinstance(subject, dict):
                continue
            m = url_re.search(subject.get("url") or "")
            if m and n.get("id") is None:
                continue
            if m:
                threads.append((str(n["id"]), int(m.group(1))))
                if meta is not None:
                    meta[str(n["id"])] = {
                        "reason": n.get("reason"),
                        "title": subject.get("title"),
                    }
        if len(raw) < PER_PAGE:
            break
    return threads


def pr_states(
    gh: Gh, numbers: set[int], repo: str = REPO
) -> tuple[dict[int, str], str | None]:
    """``({number: state}, notice)`` for ``numbers``, as far as resolvable."""
    states: dict[int, str] = {}
    notice = None
    try:
        listed = _json(
            gh,
            [
                "pr", "list", "--repo", repo, "--state", "all",
                "--json", "number,state", "--limit", str(PR_LIST_LIMIT),
            ],
        )  # fmt: skip
        for pr in listed or []:
            states[int(pr["number"])] = pr.get("state")
    except (RuntimeError, ValueError, KeyError, TypeError) as e:
        notice = f"PR list failed ({e})"
    for num in sorted(numbers - set(states))[:MAX_LOOKUPS]:
        try:
            pr = _json(gh, ["pr", "view", str(num), "--repo", repo, "--json", "state"])
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


def unsubscribes(thread: dict, keep_subscription: set[int], settled: bool) -> bool:
    """Whether a selected thread is ignored before it is marked done.

    **Unsubscribe only on merge, and only on the ``--pr`` path.** A
    still-open PR's subscription is what re-creates its thread on new
    activity, and a closed PR can be reopened, so both keep it: a ``--pr``
    caller names its non-merged PRs in ``keep_subscription``. Settled mode
    never ignores — the merge tail already unsubscribed each merged PR's
    thread and ``init-pr`` muted it at creation, so the sweep is one
    ``DELETE`` per thread."""
    return not settled and thread["pr"] not in keep_subscription


def hold(selected: list[dict], meta: dict[str, dict]) -> tuple[list[dict], list[dict]]:
    """``(to dismiss, held)``: a thread whose reason is in
    ``ATTENTION_REASONS`` is held, carrying its reason and title, so the
    post-merge tail can answer it before anything marks it done."""
    keep: list[dict] = []
    held: list[dict] = []
    for t in selected:
        info = meta.get(t["id"], {})
        if info.get("reason") in ATTENTION_REASONS:
            held.append({**t, "reason": info["reason"], "title": info.get("title")})
        else:
            keep.append(t)
    return keep, held


def dismiss(
    gh: Gh,
    prs: set[int] | None,
    apply: bool,
    repo: str = REPO,
    keep_subscription: set[int] = frozenset(),
    hold_attention: bool = False,
) -> tuple[int, dict]:
    """The whole act; ``prs`` None is settled mode. Threads on PRs in
    ``keep_subscription``, and every settled-mode thread, are marked done
    without being ignored (:func:`unsubscribes`). ``hold_attention`` (``--pr``
    mode only) leaves a thread that wants a reply in the inbox and lists it in
    ``held`` (:func:`hold`)."""
    if hold_attention and prs is None:
        raise ValueError("hold_attention applies only to named PRs")
    settled = prs is None
    result: dict = {
        "mode": "settled" if prs is None else "pr",
        "dry_run": not apply,
        "threads": 0,
        "selected": [],
        "held": [],
        "dismissed": 0,
        "unsubscribed": 0,
        "failed": [],
        "unsubscribe_failed": [],
        "error": None,
        "unresolved": [],
        "notice": None,
    }
    meta: dict[str, dict] = {}
    try:
        threads = list_threads(gh, repo, meta)
    except (RuntimeError, ValueError, KeyError, TypeError) as e:
        result["notice"] = f"notification list failed ({e})"
        return 1, result
    result["threads"] = len(threads)
    states: dict[int, str] = {}
    if settled:
        states, result["notice"] = pr_states(gh, {num for _, num in threads}, repo)
    result["selected"], result["unresolved"] = select(threads, states, prs)
    if hold_attention:
        result["selected"], result["held"] = hold(result["selected"], meta)
    if not apply:
        result["would_keep"] = sum(
            not unsubscribes(t, keep_subscription, settled) for t in result["selected"]
        )
        return 0, result
    for t in result["selected"]:
        # Ignore first: the subscription is what lets a later event on the PR
        # re-create the thread once it is marked done. Not a DELETE on the
        # subscription, which a watcher of the repository outlives.
        thread = f"/notifications/threads/{t['id']}"
        calls = [(["DELETE", thread], "dismissed", "failed")]
        if unsubscribes(t, keep_subscription, settled):
            calls.insert(
                0,
                (
                    ["PUT", f"{thread}/subscription", "-F", "ignored=true"],
                    "unsubscribed",
                    "unsubscribe_failed",
                ),
            )
        for args, count, failed in calls:
            rc, _, err = gh(["api", "-X", *args])
            if rc == 0:
                result[count] += 1
            else:
                result[failed].append(t["id"])
                result["error"] = result["error"] or first_line(err) or f"exit {rc}"
    return 0, result


def summary(result: dict) -> str:
    """One report line, for a caller that prints rather than parses."""
    n = len(result["selected"])
    if result["dry_run"]:
        kept = result.get("would_keep", 0)
        line = f"would unsubscribe and dismiss {n} notification(s)"
        if result["mode"] == "settled":
            line = f"would dismiss {n} notification(s)"
        elif kept:
            line = (
                f"would dismiss {n} notification(s), unsubscribing {n - kept}"
                f" ({kept} kept subscribed: not merged)"
            )
    else:
        line = (
            f"dismissed {result['dismissed']} notification(s), "
            f"unsubscribed {result['unsubscribed']}"
        )
        failed = len(result["failed"]) + len(result["unsubscribe_failed"])
        if failed:
            line += f", {failed} call(s) failed"
    if result.get("held"):
        line += f"; {len(result['held'])} held for a reply"
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
    p.add_argument(
        "--keep-subscription",
        action="store_true",
        help="with --pr: mark done only, never ignore (the PRs are not merged)",
    )
    p.add_argument(
        "--hold-attention",
        action="store_true",
        help="with --pr: list, never dismiss, a thread that wants a reply",
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
    if (args.keep_subscription or args.hold_attention) and args.settled:
        sys.exit("--keep-subscription and --hold-attention apply only with --pr")
    prs = None if args.settled else set(args.pr)
    keep = prs if args.keep_subscription else set()
    rc, result = dismiss(
        real_gh,
        prs,
        args.apply,
        keep_subscription=keep,
        hold_attention=args.hold_attention,
    )
    json.dump(result if args.full else compact(result), sys.stdout, indent=2)
    sys.stdout.write("\n")
    return rc


if __name__ == "__main__":
    sys.exit(main())
