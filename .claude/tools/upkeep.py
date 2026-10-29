#!/usr/bin/env python3
# cspell:word defang
"""The model-free morning upkeep pass — what the retired ``housekeeping`` skill
did, composed from the committed helpers it drove one call at a time. The
``plan`` pre-launch runs it; its approvals ride the launcher's one prune ask.

Usage (from the base repo root; a worktree is refused):

    python3 .claude/tools/upkeep.py run [--json] [--no-doc] [--if-stale]
    python3 .claude/tools/upkeep.py ran-today
    python3 .claude/tools/upkeep.py prune [--apply] [--json]
    python3 .claude/tools/upkeep.py fold-stamp --batches N [--folds M]
    python3 .claude/tools/upkeep.py fold-today

``prune`` runs steps 4–7 alone, disarmed unless ``--apply`` — the operator's
by-hand cleanup on a machine whose pass is not armed, and the launcher's apply
on the operator's yes, applying the one rule below rather than a second one
that drifts.

``fold-stamp`` is the launcher's record that the day's lever fold ran, written
beside this pass's stamp; ``fold-today`` prints the scheduler's line for it
(``fold ran at HH:MM, N batches[, M folded into open issues]`` or
``no fold today``). Neither reads the network.

``run`` executes twelve steps in order, each under its own timeout and each
**best-effort**: a failure is a report line, never an abort of later steps.
The one exception is step 1 — running from anywhere but the base checkout
refuses the whole pass, since every later step assumes it.

 1. confirm the base repo root;
 2. fast-forward main (on failure, continue on the checked-out commit);
 3. upgrade the Claude Code CLI cask;
 4. read the terminal PR set (merged AND closed — a PR closed early leaves its
    branch behind too) and the open set;
 5. intersect with the board: a worktree or local branch is cleanup-eligible
    when its issue's status TYPE is completed or canceled and no PR for its
    branch is open. PR-merged alone is never enough — a merged In Review
    session is still a live conversation the fleet launcher resumes.
    A canceled issue with an open PR is flagged; a branch whose issue cannot be
    resolved is reported; Linear unreachable cleans NOTHING;
 6. prune the eligible worktrees and stray local branches, passed to
    ``prune_worktrees`` as ``--closed`` names: only a live session lock holds
    one, and the report names each one's commits ahead of ``origin/main``;
 7. dismiss every settled PR's notifications (``pr_notifications dismiss
    --settled``) — merged or closed, whatever its issue's state, one thread at
    a time (never mark-all);
 8. the open count of each parked milestone, with its direction since the
    previous stamp — the line the ``plan`` bootstrap says;
 9. convention-reference check, hook-wiring check, the skill-size watch line
    (``skill_size --utilization``), allowlist cruft, and the
    monthly refresh gate — mining the recent session digests when due and
    stamping the marker whatever the yield, writing no allow-rule;
10. the fleet cost: ``fleet_cost reconcile`` per day (Bedrock metered beside
    the bill, with its drift flag) and the seat allowance, which has no bill
    line, read with ``subscription_usage --log`` so each pass that gets a reading appends the one
    sample its per-day figure is derived from;
11. the session digest (``session_metrics digest``) over every transcript since
    the last digest's window closed, its rate-calibration line fed by step 10's
    reconcile figure and the settled day's ``fleet_cost rates`` read, and its
    CI section (``--ci``) read from the merge queue over the same window;
12. the memory scan gate, then the memory audit when due;
13. the transcript-purge dry-run manifest.

**Arming.** Step 6 runs as a dry run and touches nothing unless
``DS_UPKEEP_ARMED=1`` is exported from the untracked runtime config. Step 7 is
always armed in ``run``: with the board gate gone, a settled PR's threads are
dismissible on sight, and a dismissal destroys nothing. The open-PR guard holds
even when armed, and the purge never arms — its apply stays an operator
approval at the launcher's prune ask. ``prune`` arms both steps only under
``--apply``.

**Report.** One section of the Planning document — from ``REPORT_HEADING``
through its ``REPORT_END`` line — is replaced wholesale each run, bounded to
``REPORT_CHAR_CAP``. The same report
prints to stdout (the standalone interface, and the fallback when Linear is
unreachable); ``--json`` prints the structured twin instead. Progress — a line
as each step starts and ends, then a total — goes to stderr, so neither stdout
form changes and the launcher's terminal shows which wait it is on. A local stamp beside
the allowlist refresh marker answers ``ran-today`` with no network read, and
carries the parked counts the next pass measures direction against; it is
written only when the report reached the document, so a ``--no-doc`` preview or
a failed write never answers it. ``run --if-stale`` consults that stamp first
and skips the pass when it already answers yes — the form the ``plan`` launcher
calls, so a same-day relaunch costs no second pass. A refused splice (heading repeated, end line
gone) is permanent until the section is fixed by hand, and shows in the document
as a stale run timestamp.

Stdlib only; a skill-tool under ``.claude/tools/``, deliberately **not** a Cargo
workspace member. Tests live in ``tests/test_upkeep.py``.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

import board_batch
import fleet_cost
import linear_api
import planning_doc
import prune_conversations
import prune_worktrees
import session_digest
import subscription_usage

TOOLS = Path(__file__).resolve().parent
REPO = "DASMAC-com/dropset"
ARM_ENV = "DS_UPKEEP_ARMED"
REPORT_HEADING = "Upkeep report — machine-written, latest run only"
# The section's closing line. Splicing only between the heading and this line is
# what keeps a note another session appended below the report from being
# swallowed as part of it on the next run.
REPORT_END = "End of upkeep report."
STAMP_NAME = ".upkeep-last-run.json"
FOLD_STAMP_NAME = ".fold-last-run.json"
# The seat allowance's sample log, at most one reading per pass — machine-local beside
# the stamp, since the allowance is one operator's subscription.
SAMPLES_NAME = ".allowance-samples.jsonl"

# Roughly 1.5k tokens. The section is replaced every run, so the cap bounds what
# every planning bootstrap pays to read it, not just this run's write.
REPORT_CHAR_CAP = 6000
LIST_CAP = 6

TIMEOUT_GIT = 120
TIMEOUT_BREW = 60
TIMEOUT_GH = 60
TIMEOUT_TOOL = 120
# The transcript walk alone takes about a minute, before the Cost Explorer read.
TIMEOUT_COST = 300
# One DELETE per thread; the first armed pass meets a backlog of a hundred-odd.
TIMEOUT_DISMISS = 300
# The digest's transcript walk plus its CI read: a cold job cache is one
# `gh api` read per merge-queue run of the past week, a few hundred.
TIMEOUT_DIGEST = 600

# How many recent digests the refresh gate mines — an upper bound: the purge
# ages digests out on the transcripts' two-day rule, so once it is applied
# only the last few survive.
MINE_DIGESTS = 7

# PR list bounds. A full open page means the open set may be truncated, and the
# open-PR guard is only as good as that set, so a full page fails closed.
CLOSED_PR_LIMIT = 30
OPEN_PR_LIMIT = 100

RESOLVED_TYPES = ("completed", "canceled")
# The parking milestones, in report order. Session pending is short-lived by
# design: a count there is a seat session that died mid-review. Follow-ups is
# retired as a parking set: filers now land un-parked in the curator's pool,
# and issues still carrying it go uncounted until the milestone is deleted.
PARKED_MILESTONES = (
    "Audit findings",
    "Session pending",
)
# A branch tied to an ENG issue, by its NUMBER whatever the verb: `eng-<n>`,
# which every issue-keyed launch now creates, or a pre-scheme spelling — the
# `worktree-<n>` that `claude -w <n>` named it, and the `worktree-eng-` /
# `-ceo-` / `-exp-<n>` forms from before the bare-number rename. Tying them all
# to their issue is what makes an architect or explore worktree eligible once
# its issue is completed or canceled; the pruner then gates it by tier. A
# worded name (`worktree-exp-<topic>`) names no issue, so it is reported as
# unresolvable and cleaned by hand — never guessed at.
BRANCH_RE = re.compile(
    r"^(?:worktree-)?(?:eng-|ceo-|exp-)?(\d+)$", re.IGNORECASE | re.ASCII
)

Runner = Callable[[list[str], float, "str | None"], "tuple[int, str, str]"]


def real_run(cmd: list[str], timeout: float, cwd: str | None = None):
    """``(rc, stdout, stderr)``; a timeout or a missing binary is an rc, not a
    raise, so one hung step reads as one report line."""
    try:
        proc = subprocess.run(
            cmd, capture_output=True, text=True, timeout=timeout, cwd=cwd, check=False
        )
    except subprocess.TimeoutExpired:
        return 124, "", f"timed out after {timeout:g}s"
    except OSError as e:
        return 127, "", str(e)
    return proc.returncode, proc.stdout, proc.stderr


def first_line(text: str) -> str:
    return (text.strip().splitlines() or [""])[0][:160]


@dataclass
class Ctx:
    base: Path
    armed: bool
    now: datetime
    # The notifications step's own arming: the daily pass dismisses for real
    # whatever `armed` says, `prune` only under `--apply`.
    dismiss: bool = False
    run: Runner = real_run
    # numbers -> {number: status type}; raises when Linear is unreachable.
    lookup: Callable[[list[int]], dict[int, str]] | None = None
    # () -> {milestone: open count}; raises when Linear is unreachable.
    parked: Callable[[], dict[str, int]] | None = None
    steps: list[dict] = field(default_factory=list)
    data: dict = field(default_factory=dict)
    # Where the per-step progress lines go — stderr from the CLI, None (silent)
    # under test — and the clock their elapsed seconds are read from.
    progress: Callable[[str], None] | None = None
    clock: Callable[[], float] = time.monotonic

    def tool(self, name: str, *args: str, timeout: float = TIMEOUT_TOOL):
        return self.run(
            [sys.executable, str(TOOLS / name), *args], timeout, str(self.base)
        )

    def record(self, step: str, line: str, ok: bool = True, **extra) -> None:
        self.steps.append({"step": step, "ok": ok, "line": line, **extra})


# --- eligibility -----------------------------------------------------------


def pr_state_by_branch(prs: list[dict]) -> dict[str, str]:
    """One state per head branch: OPEN beats MERGED beats CLOSED, so any open PR
    on a branch keeps it — the guard that holds even when armed."""
    rank = {"OPEN": 3, "MERGED": 2, "CLOSED": 1}
    out: dict[str, str] = {}
    for pr in prs:
        branch, state = pr.get("headRefName"), pr.get("state")
        if branch and rank.get(state, 0) > rank.get(out.get(branch), 0):
            out[branch] = state
    return out


def classify(pr_state: str | None, status_type: str | None) -> tuple[bool, str | None]:
    """``(eligible, flag)`` for one branch. Fail closed everywhere."""
    if pr_state == "OPEN":
        flag = "canceled issue with an open PR" if status_type == "canceled" else None
        return False, flag
    if status_type is None:
        return False, "issue unresolvable"
    return status_type in RESOLVED_TYPES, None


def eligible_branches(
    candidates: set[str],
    pr_states: dict[str, str],
    status_by_number: dict[int, str],
) -> tuple[set[str], list[str]]:
    """The cleanup-eligible subset of ``candidates`` plus ``branch: flag`` lines."""
    eligible: set[str] = set()
    flags: list[str] = []
    for branch in sorted(candidates):
        m = BRANCH_RE.match(branch)
        status = status_by_number.get(int(m.group(1))) if m else None
        ok, flag = classify(pr_states.get(branch), status)
        if ok:
            eligible.add(branch)
        if flag:
            flags.append(f"{branch}: {flag}")
    return eligible, flags


def linear_lookup(numbers: list[int]) -> dict[int, str]:
    """Status type per ENG number, through the bare-key GraphQL path."""
    key = linear_api.env_var("LINEAR_API_KEY")
    project = linear_api.env_var("LINEAR_PROJECT_ID")
    out: dict[int, str] = {}
    for issue in board_batch.fetch_issues_by_number(key, project, numbers):
        # Numbers are per-team; only the ENG team's issue answers for eng-###.
        ident = str(issue.get("identifier") or "")
        if ident.startswith("ENG-"):
            out[int(issue["number"])] = (issue.get("state") or {}).get("type")
    return out


def linear_parked() -> dict[str, int]:
    """Open issues per parking milestone, in one read."""
    key = linear_api.env_var("LINEAR_API_KEY")
    project = linear_api.env_var("LINEAR_PROJECT_ID")
    counts = dict.fromkeys(PARKED_MILESTONES, 0)
    for issue in board_batch.fetch_open_in_milestones(
        key, project, list(PARKED_MILESTONES)
    ):
        name = (issue.get("projectMilestone") or {}).get("name")
        if name in counts:
            counts[name] += 1
    return counts


# --- steps -----------------------------------------------------------------


def check_base(run: Runner, cwd: str) -> tuple[Path | None, list[dict], str]:
    """``(base, trees, why)``; ``base`` is None when cwd is not the base root."""
    rc, out, err = run(["git", "worktree", "list", "--porcelain"], 30, cwd)
    if rc != 0:
        return None, [], f"git worktree list failed: {first_line(err)}"
    trees = prune_worktrees.parse_worktrees(out)
    bases = [t for t in trees if prune_worktrees.is_base(t)]
    rc, top, _ = run(["git", "rev-parse", "--show-toplevel"], 30, cwd)
    if not bases or rc != 0:
        return None, trees, "main is not checked out in any worktree"
    base = Path(bases[0]["path"]).resolve()
    if Path(top.strip()).resolve() != base:
        return None, trees, f"run from the base repo root ({base}), not a worktree"
    return base, trees, ""


def step_pull(ctx: Ctx) -> None:
    rc, out, err = ctx.run(["git", "pull", "--ff-only"], TIMEOUT_GIT, str(ctx.base))
    if rc == 0:
        line = (
            "main already current"
            if "Already up to date" in out
            else "main fast-forwarded"
        )
        ctx.record("pull", line)
        return
    _, sha, _ = ctx.run(["git", "rev-parse", "--short", "HEAD"], 30, str(ctx.base))
    ctx.record(
        "pull", f"fast-forward failed ({first_line(err)}); on {sha.strip()}", False
    )


def step_cli(ctx: Ctx) -> None:
    rc, _, err = ctx.run(
        ["brew", "upgrade", "--cask", "claude-code@latest"], TIMEOUT_BREW, None
    )
    if rc == 0:
        ctx.record("cli", "CLI cask upgraded or already current")
    else:
        ctx.record("cli", f"CLI upgrade failed: {first_line(err)}", False)


def _gh_json(ctx: Ctx, args: list[str]):
    rc, out, err = ctx.run(["gh", *args], TIMEOUT_GH, str(ctx.base))
    if rc != 0:
        raise RuntimeError(first_line(err) or f"gh exited {rc}")
    return json.loads(out or "null")


def step_prs(ctx: Ctx) -> None:
    fields = "number,headRefName,state"
    try:
        closed = _gh_json(
            ctx,
            [
                "pr",
                "list",
                "--repo",
                REPO,
                "--state",
                "closed",
                "--json",
                fields,
                "--limit",
                str(CLOSED_PR_LIMIT),
            ],
        )
        opened = _gh_json(
            ctx,
            [
                "pr",
                "list",
                "--repo",
                REPO,
                "--state",
                "open",
                "--json",
                fields,
                "--limit",
                str(OPEN_PR_LIMIT),
            ],
        )
        if len(opened or []) >= OPEN_PR_LIMIT:
            raise RuntimeError(f"open PR list hit its {OPEN_PR_LIMIT} limit")
    except (RuntimeError, ValueError) as e:
        ctx.record("prs", f"PR list failed ({e}); nothing is cleanup-eligible", False)
        return
    ctx.data["prs"] = list(closed or []) + list(opened or [])
    ctx.data["open_branches"] = sorted(
        p["headRefName"] for p in opened or [] if p.get("headRefName")
    )
    ctx.record("prs", f"{len(closed or [])} terminal PR(s), {len(opened or [])} open")


def step_board(ctx: Ctx) -> None:
    ctx.data["eligible"] = set()
    if "prs" not in ctx.data:
        ctx.record("board", "no PR set, so nothing is cleanup-eligible", False)
        return
    rc, refs, _ = ctx.run(
        ["git", "for-each-ref", "--format=%(refname:short)", "refs/heads"],
        30,
        str(ctx.base),
    )
    local = {b for b in refs.split() if BRANCH_RE.match(b)} if rc == 0 else set()
    flags = [] if rc == 0 else ["local branch list failed; strays not considered"]
    in_trees = {t["branch"] for t in ctx.data.get("trees", []) if t.get("branch")}
    candidates = (local | in_trees) - {"main"}
    numbers = sorted({int(m.group(1)) for b in candidates if (m := BRANCH_RE.match(b))})
    try:
        status = ctx.lookup(numbers) if numbers else {}
    except Exception as e:  # noqa: BLE001 — any lookup failure fails closed
        ctx.record(
            "board",
            f"Linear unreachable ({first_line(str(e))}); cleaned nothing",
            False,
        )
        return
    eligible, branch_flags = eligible_branches(
        candidates, pr_state_by_branch(ctx.data["prs"]), status
    )
    flags += branch_flags
    ctx.data["eligible"] = eligible
    ctx.data["local_traces"] = local | in_trees
    ctx.record(
        "board",
        f"{len(candidates)} candidate(s), {len(eligible)} cleanup-eligible",
        flags=flags,
    )


def step_prune(ctx: Ctx) -> None:
    targets = sorted(
        ctx.data.get("eligible", set()) & ctx.data.get("local_traces", set())
    )
    if not targets:
        ctx.record("prune", "no worktree or branch to clean")
        return
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as f:
        f.write("\n".join(targets))
        closed_file = f.name
    try:
        # Every target's issue is completed or canceled, so it goes as a closed
        # name: the unpushed and no-upstream holds that kept a stray branch
        # forever are lifted, and only a live session lock holds.
        args = ["--closed-file", closed_file] + ([] if ctx.armed else ["--dry-run"])
        rc, out, err = ctx.tool("prune_worktrees.py", *args)
    finally:
        os.unlink(closed_file)
    if rc != 0:
        ctx.record("prune", f"pruner failed: {first_line(err)}", False)
        return
    result = json.loads(out)
    ctx.data["pruned"] = result
    verb = "removed" if ctx.armed else "would remove"
    ctx.record(
        "prune",
        f"{verb} {len(result['removed'])} worktree(s) and "
        f"{len(result.get('branches_removed', []))} stray branch(es); "
        f"{len(result['skipped'])} held back",
        skipped=[f"{s['branch']}: {s['reason']}" for s in result["skipped"]],
        ahead=[
            f"{b}: {a['ahead'] if a.get('ahead') is not None else '?'} ahead of "
            f"origin/main, tip {(a.get('tip') or '?')[:12]}"
            for b, a in sorted(result.get("ahead", {}).items())
        ],
        unmatched=[
            f"{b}: matches no worktree or branch" for b in result.get("unmatched", [])
        ],
    )


def step_notifications(ctx: Ctx) -> None:
    # No board gate: a merged or closed PR's threads are settled whatever its
    # issue's state, and an open PR's are never selected. Armed on its own
    # flag, since dismissal destroys nothing the way a prune can.
    args = ["dismiss", "--settled"] + (["--apply"] if ctx.dismiss else [])
    rc, out, err = ctx.tool("pr_notifications.py", *args, timeout=TIMEOUT_DISMISS)
    try:
        result = json.loads(out)
    except ValueError:
        result = None
    if not isinstance(result, dict) or "summary" not in result:
        ctx.record(
            "notifications",
            f"dismissal tool failed: {first_line(err) or f'exit {rc}'}",
            False,
        )
        return
    ctx.record(
        "notifications",
        result["summary"],
        ok=rc == 0 and not result.get("failed") and not result.get("notice"),
    )


def direction(count: int, before: int | None) -> str:
    if before is None:
        return "no previous count"
    if count == before:
        return "unchanged"
    return f"{'up' if count > before else 'down'} {abs(count - before)}"


def step_parked(ctx: Ctx) -> None:
    if ctx.parked is None:
        ctx.record("parked", "no parked-count lookup configured", False)
        return
    try:
        counts = ctx.parked()
    except Exception as e:  # noqa: BLE001 — a failed read is a line, not a crash
        ctx.record("parked", f"Linear unreachable ({first_line(str(e))})", False)
        return
    previous = ctx.data.get("previous_parked") or {}
    ctx.data["parked"] = counts
    ctx.record(
        "parked",
        ", ".join(
            f"{name} {counts.get(name, 0)} "
            f"({direction(counts.get(name, 0), previous.get(name))})"
            for name in PARKED_MILESTONES
        ),
    )


def _json_tool(
    ctx: Ctx,
    step: str,
    name: str,
    *args: str,
    ok_rcs=(0, 1),
    timeout: float = TIMEOUT_TOOL,
):
    rc, out, err = ctx.tool(name, *args, timeout=timeout)
    if rc not in ok_rcs:
        ctx.record(step, f"{name} failed: {first_line(err)}", False)
        return None
    try:
        return json.loads(out)
    except ValueError:
        ctx.record(step, f"{name} printed no JSON", False)
        return None


def recent_digests(base: Path, limit: int = MINE_DIGESTS) -> list[Path]:
    """The newest digest twins on disk, newest first (their names sort by
    date)."""
    directory = base / session_digest.DIGEST_DIR
    return sorted(directory.glob(session_digest.DIGEST_GLOB), reverse=True)[:limit]


def mine_refresh_candidates(ctx: Ctx) -> tuple[list[str], int]:
    """``(candidates, sessions_read)``: uncovered repeated Bash shapes across
    the sessions the digests on disk cover, most frequent first, and how many
    sessions that was. The digest already aggregated every transcript, so
    nothing is re-run per session. A session spanning digests counts once,
    from the newest, whose candidates are its whole session's. Read-only: the
    candidates go to the report, never into settings."""
    counts: dict[str, int] = {}
    seen: set[str] = set()
    for path in recent_digests(ctx.base):
        try:
            twin = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        for session in twin.get("sessions", []) if isinstance(twin, dict) else []:
            sid = session.get("session_id")
            if not sid or sid in seen:
                continue
            seen.add(sid)
            for c in session.get("hardening_candidates", []):
                if c.get("cost_kind") == "prompt-churn":
                    counts[c["signature"]] = counts.get(c["signature"], 0) + c["count"]
    read = len(seen)
    ranked = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))
    return [f"{sig} ({n} calls)" for sig, n in ranked], read


def step_checks(ctx: Ctx) -> None:
    refs = _json_tool(ctx, "convention-refs", "convention_refs.py", "--json")
    if refs is not None:
        ctx.record(
            "convention-refs",
            f"{refs['count']} dangling reference(s) of {refs['checked']}",
            dangling=[
                f"{d['citer']} -> {d['target']} ({d['kind']})" for d in refs["dangling"]
            ],
        )
    hooks = _json_tool(ctx, "hook-wiring", "hook_wiring.py", "--json")
    if hooks is not None:
        inert = sorted(
            set(hooks.get("unwired", []))
            | set(hooks.get("mismatched", {}))
            | set(hooks.get("misdirected", {}))
        )
        ctx.record("hook-wiring", f"{len(inert)} inert guard(s)", inert=inert)
        # The private-key deny rules are the guard's other layer; a missing one
        # is the same gap as an unwired guard, so it gets its own line.
        missing = hooks.get("deny_missing", [])
        ctx.record(
            "deny-rules",
            f"{len(missing)} missing private-key deny rule(s)",
            missing=missing,
        )

    # Exit 1 is the watch line being crossed, a finding rather than a failure —
    # but only when the tool printed the rows that say so; an exit 1 with no
    # flagged row is a broken run, and must not read as "nothing near".
    rc, out, err = ctx.tool("skill_size.py", "--utilization")
    near = [ln[1:].strip() for ln in out.splitlines() if ln.startswith("!")]
    if rc == 0 or (rc == 1 and near):
        ctx.record(
            "skill-size",
            f"{len(near)} resident file(s) near their byte limit",
            near=near,
        )
    else:
        ctx.record("skill-size", f"skill-size check failed: {first_line(err)}", False)

    cruft = _json_tool(ctx, "allowlist", "allowlist.py", "cruft")
    if cruft is not None:
        ctx.record(
            "allowlist",
            # `count` is the whole allowlist; the shortlist is `flagged`.
            f"{len(cruft['flagged'])} cruft entry(ies) of {cruft['count']} rule(s)",
            cruft=[f"{f['rule']} ({f['category']})" for f in cruft["flagged"]],
        )
    due = _json_tool(ctx, "allowlist-refresh", "allowlist.py", "refresh-due")
    if due is None:
        return
    if not due.get("due"):
        ctx.record("allowlist-refresh", f"refresh not due ({due.get('reason')})")
        return
    candidates, read = mine_refresh_candidates(ctx)
    if read == 0:
        # A miner that read nothing is broken, not a zero yield — stamping it
        # would hide the failure for a whole month.
        ctx.record(
            "allowlist-refresh",
            "refresh due; no digested session mined, not stamped",
            False,
        )
        return
    rc, _, err = ctx.tool("allowlist.py", "refresh-record", "--added", "0")
    stamped = "stamped" if rc == 0 else f"stamp failed: {first_line(err)}"
    ctx.record(
        "allowlist-refresh",
        f"refresh due; {len(candidates)} uncovered shape(s) mined from "
        f"{read} session(s); {stamped}",
        ok=rc == 0,
        candidates=candidates,
    )


def cost_day_line(day: str, summary: dict) -> str:
    """One reconciled day: Bedrock against its bill, the seat as metered alone."""
    rec = summary.get("reconcile") or {}
    seat = summary["substrates"].get(fleet_cost.SEAT) or {}
    mark = (
        " (provisional)"
        if summary.get("provisional")
        else " FLAGGED"
        if rec.get("flagged")
        else ""
    )
    line = (
        f"{day}: Bedrock {fleet_cost.money(rec.get('metered', 0.0))} metered "
        f"({fleet_cost.money(rec.get('expected', 0.0))} with the classifier) vs "
        f"{fleet_cost.money(rec.get('billed_matched', 0.0))} billed, drift "
        f"{fleet_cost.pct(rec.get('drift'))}{mark}; seat "
        f"{fleet_cost.money(seat.get('dollars', 0.0))} at Bedrock rates over "
        f"{seat.get('sessions', 0)} session(s), no bill line"
    )
    friction = summary.get("friction")
    if friction:
        line += "; friction: " + fleet_cost.render_friction(friction)
    return line


def step_cost(ctx: Ctx) -> None:
    # Exit 1 is a settled day flagged: a finding the report carries, not a
    # failed read. Exit 2 (an AWS error, expired credentials included) is.
    report = _json_tool(
        ctx, "fleet-cost", "fleet_cost.py", "reconcile", "--json", timeout=TIMEOUT_COST
    )
    if report is not None:
        ctx.data["fleet_cost"] = report
        days = sorted(report.get("days", {}).items())
        flagged = [
            d
            for d, s in days
            if not s.get("provisional") and (s.get("reconcile") or {}).get("flagged")
        ]
        # An unreadable transcript drops its spend from the meter, so a low
        # metered total would otherwise pass as a clean day.
        unreadable = report.get("unreadable_transcripts") or 0
        line = (
            f"{len(flagged)} settled day(s) flagged past "
            f"{report.get('tolerance', 0):.0%} drift"
        )
        if unreadable:
            line += f"; {unreadable} unreadable transcript(s) not metered"
        ctx.record(
            "fleet-cost",
            line if days else "reconcile reported no day",
            ok=bool(days) and not unreadable,
            days=[cost_day_line(d, s) for d, s in days],
        )
    rc, out, err = ctx.tool(
        "subscription_usage.py", "--log", str(ctx.base / ".claude" / SAMPLES_NAME)
    )
    lines = [ln for ln in out.splitlines() if ln.strip()]
    if rc == 0:
        # Exit 0 also covers the no-credential and stale-token non-readings,
        # which print one line and sample nothing; only a logged reading
        # prints the marker.
        sampled = subscription_usage.SAMPLE_LOGGED in lines
        lines = [ln for ln in lines if ln != subscription_usage.SAMPLE_LOGGED]
        trouble = [
            ln
            for ln in lines
            if ln.startswith(("sample not logged", "sample log unreadable"))
        ]
        summary = (
            "seat allowance, sampled for the per-day figure"
            if sampled
            else f"no reading, nothing sampled: {first_line(out)}"
        )
        # The per-day figure and any log trouble lead, so the list cap can only
        # ever cut a limit row.
        ctx.record(
            "allowance",
            summary,
            ok=sampled and not trouble,
            rows=[ln for ln in lines if ln not in trouble and " per day " in ln]
            + trouble
            + [ln for ln in lines if ln not in trouble and " per day " not in ln],
        )
    else:
        reason = first_line(out) or first_line(err) or f"exit {rc}"
        ctx.record("allowance", f"allowance read failed: {reason}", False)


def digest_calibration(ctx: Ctx) -> dict:
    """The digest's rate-calibration input: the cost step's reconcile figure
    for the last settled day, and that day's billed per-million rates."""
    report = ctx.data.get("fleet_cost")
    if report is None:
        return {"error": "the fleet-cost read failed this pass"}
    days = report.get("days") or {}
    settled = sorted(d for d, s in days.items() if not s.get("provisional"))
    if not settled:
        return {"error": "the fleet-cost read reported no settled day"}
    day = settled[-1]
    out = {
        "day": day,
        "tolerance": report.get("tolerance", fleet_cost.DEFAULT_TOLERANCE),
        "reconcile": days[day].get("reconcile") or {},
    }
    # Exit 1 is a row that misses its bill: a finding the digest flags.
    rc, stdout, err = ctx.tool(
        "fleet_cost.py", "rates", day, "--json", timeout=TIMEOUT_COST
    )
    try:
        if rc not in (0, 1):
            raise ValueError(first_line(err) or f"exit {rc}")
        out["rates"] = json.loads(stdout)
    except ValueError as e:
        out["rates_error"] = f"fleet_cost rates failed: {first_line(str(e))}"
    return out


def step_digest(ctx: Ctx) -> None:
    """Write the day's session digest (``session_metrics.py digest``) over
    every transcript since the last digest closed its window; the previous
    stamped pass seeds the first one."""
    # Built before the file exists, so a raise here leaves nothing behind.
    calibration = digest_calibration(ctx)
    with tempfile.NamedTemporaryFile(
        "w", suffix=".json", prefix="upkeep-calibration-", delete=False
    ) as handle:
        json.dump(calibration, handle)
    args = ["digest", "--base", str(ctx.base), "--calibration", handle.name, "--ci"]
    if ctx.data.get("previous_run"):
        args += ["--since", ctx.data["previous_run"]]
    try:
        summary = _json_tool(
            ctx,
            "digest",
            "session_metrics.py",
            *args,
            ok_rcs=(0,),
            timeout=TIMEOUT_DIGEST,
        )
    finally:
        os.unlink(handle.name)
    if summary is None:
        return
    path = Path(summary.get("path") or "")
    if path.is_relative_to(ctx.base):
        path = path.relative_to(ctx.base)
    ctx.record(
        "digest",
        f"{summary.get('sessions', 0)} session(s), "
        f"Bedrock {fleet_cost.money(summary.get('bedrock', 0.0))}, seat "
        f"{fleet_cost.money(summary.get('seat', 0.0))} {fleet_cost.sm.NOT_BILLED}, "
        f"{summary.get('flagged', 0)} flagged line(s); {path}",
        ok=bool(summary.get("path")),
    )


def step_memory(ctx: Ctx) -> None:
    memory_dir = (
        prune_conversations.projects_root()
        / prune_conversations.slugify(ctx.base)
        / "memory"
    )
    gate = _json_tool(
        ctx, "memory", "memory_scan_gate.py", "check", str(memory_dir), ok_rcs=(0,)
    )
    if gate is None:
        return
    if not gate.get("scan"):
        ctx.record("memory", f"memory audit not due ({gate.get('reason')})")
        return
    rc, out, err = ctx.tool(
        "memory_audit.py", str(memory_dir), "--repo-root", str(ctx.base)
    )
    if rc != 0:
        ctx.record("memory", f"memory audit failed: {first_line(err)}", False)
        return
    # The audit closes with its own `memory-audit | …` summary; only the lines
    # above it are findings.
    findings = [
        ln
        for ln in out.splitlines()
        if ln.strip() and not ln.startswith("memory-audit |")
    ]
    rc, _, err = ctx.tool("memory_scan_gate.py", "record", str(memory_dir))
    stamp = "" if rc == 0 else f"; gate stamp failed: {first_line(err)}"
    ctx.record(
        "memory",
        f"memory audit ran: {len(findings)} finding(s){stamp}",
        ok=rc == 0,
        findings=findings,
    )


def step_purge(ctx: Ctx) -> None:
    args = ["--dropset-repo", str(ctx.base)]
    for branch in ctx.data.get("open_branches", []):
        args += ["--protected-branch", branch]
    # Only an ACTUAL removal frees a slug early; a dry run's worktrees still
    # exist, and the purge keeps any existing worktree's slug anyway.
    pruned = ctx.data.get("pruned") or {}
    if not pruned.get("dry_run", True):
        for r in pruned.get("removed", []):
            # One token with `=`: a slug of an absolute path starts with "-",
            # which argparse would read as a flag, not as this option's value.
            slug = prune_conversations.slugify(Path(r["path"]))
            args.append(f"--completed-slug={slug}")
    rc, out, err = ctx.tool("prune_conversations.py", *args)
    if rc != 0:
        ctx.record("purge", f"purge dry-run failed: {first_line(err)}", False)
        return
    total = next(
        (
            ln.split(":", 1)[1].strip()
            for ln in out.splitlines()
            if "TOTAL to free" in ln
        ),
        "nothing",
    )
    ctx.record("purge", f"purge dry-run: {total} to free (apply is the launcher's ask)")


STEPS = (
    ("pull", step_pull),
    ("cli", step_cli),
    ("prs", step_prs),
    ("board", step_board),
    ("prune", step_prune),
    ("notifications", step_notifications),
    ("parked", step_parked),
    ("checks", step_checks),
    ("cost", step_cost),
    ("digest", step_digest),
    ("memory", step_memory),
    ("purge", step_purge),
)

# What each step's progress line names, and the timeout bounding its slowest
# call — so a terminal sitting on one line for minutes says which wait it is.
STEP_LABELS = {
    "pull": ("fast-forward main", f"per-call timeout {TIMEOUT_GIT}s"),
    "cli": ("upgrade the Claude Code CLI", f"per-call timeout {TIMEOUT_BREW}s"),
    "prs": ("terminal and open PR sets", f"per-call timeout {TIMEOUT_GH}s"),
    "board": (
        "the board's verdict per branch",
        f"per-call timeout {linear_api.REQUEST_TIMEOUT}s",
    ),
    "prune": ("prune worktrees and branches", f"per-call timeout {TIMEOUT_TOOL}s"),
    "notifications": (
        "dismiss settled PR notifications",
        f"timeout {TIMEOUT_DISMISS}s",
    ),
    "parked": (
        "parked milestone counts",
        f"per-call timeout {linear_api.REQUEST_TIMEOUT}s",
    ),
    "checks": ("convention and wiring checks", f"per-call timeout {TIMEOUT_TOOL}s"),
    "cost": ("fleet cost and allowance", f"per-call timeout {TIMEOUT_COST}s"),
    "digest": ("session digest", f"per-call timeout {TIMEOUT_DIGEST}s"),
    "memory": ("memory scan gate and audit", f"per-call timeout {TIMEOUT_TOOL}s"),
    "purge": ("transcript-purge dry run", f"per-call timeout {TIMEOUT_TOOL}s"),
}


# The cleanup half alone — the `prune` subcommand, so a by-hand prune and the
# daily pass apply ONE survival rule rather than two that drift.
PRUNE_STEPS = ("prs", "board", "prune", "notifications")


def step_start_line(index: int, total: int, name: str) -> str:
    what, bound = STEP_LABELS.get(name, (name, "no timeout recorded"))
    return f"upkeep: [{index}/{total}] {name} — {what} ({bound})"


def step_end_line(
    index: int, total: int, name: str, elapsed: float, crashed: bool
) -> str:
    verdict = "crashed" if crashed else "done"
    return f"upkeep: [{index}/{total}] {name} — {verdict} in {elapsed:.1f}s"


def total_line(total: int, elapsed: float, crashed: int) -> str:
    tail = f", {crashed} crashed" if crashed else ""
    return f"upkeep: {total} step(s) in {elapsed:.1f}s{tail}"


def run_pass(ctx: Ctx, only: tuple[str, ...] | None = None) -> dict:
    # Progress goes to `ctx.progress` (stderr), never stdout, which stays the
    # report or its JSON twin byte for byte.
    say = ctx.progress or (lambda _line: None)
    chosen = [(n, fn) for n, fn in STEPS if only is None or n in only]
    began = ctx.clock()
    crashes = 0
    for index, (name, fn) in enumerate(chosen, 1):
        say(step_start_line(index, len(chosen), name))
        start = ctx.clock()
        crashed = False
        try:
            fn(ctx)
        except Exception as e:  # noqa: BLE001 — best-effort: a crash is a line
            crashed = True
            crashes += 1
            ctx.record(
                name, f"step crashed: {type(e).__name__}: {first_line(str(e))}", False
            )
        say(step_end_line(index, len(chosen), name, ctx.clock() - start, crashed))
    say(total_line(len(chosen), ctx.clock() - began, crashes))
    result = {
        "ran_at": ctx.now.isoformat(),
        "armed": ctx.armed,
        "steps": ctx.steps,
    }
    if "parked" in ctx.data:
        result["parked"] = ctx.data["parked"]
    return result


# --- report ----------------------------------------------------------------

_BASENAME_DOT = re.compile(r"(?<=[\w-])\.(?=[A-Za-z][\w-]*)")


def defang(text: str) -> str:
    """Make a line safe for the Planning document's write-mangle rules.

    Linear linkifies a hostname-valid ``name.ext`` and pairs stray asterisks into
    emphasis, so the doc copy swaps the dot for a one-dot leader (U+2024) and the
    asterisk for an operator asterisk (U+2217) — visually the same, inert to the
    markdown. Backticks are dropped (no code spans). Stdout keeps the real text.
    """
    text = text.replace("`", "")
    text = text.replace("*", "∗")
    return _BASENAME_DOT.sub("․", text)


LIST_KEYS = (
    "near",
    "flags",
    "skipped",
    "ahead",
    "unmatched",
    "dangling",
    "inert",
    "cruft",
    "candidates",
    "findings",
    "days",
    "rows",
)


def one_line(text: object) -> str:
    """Collapse whitespace, so a multi-line git error cannot inject a line that
    reads as a heading — or as the section's end marker — into the report."""
    return " ".join(str(text).split())


def render(result: dict, cap: int = REPORT_CHAR_CAP) -> str:
    mode = "armed" if result["armed"] else "dry run (disarmed)"
    lines = [f"Run {result['ran_at']}, {mode}.", ""]
    for s in result["steps"]:
        mark = "" if s["ok"] else " (failed)"
        lines.append(f"- {s['step']}{mark}: {one_line(s['line'])}")
        for key in LIST_KEYS:
            items = s.get(key) or []
            for item in items[:LIST_CAP]:
                lines.append(f"  - {one_line(item)}")
            if len(items) > LIST_CAP:
                lines.append(f"  - and {len(items) - LIST_CAP} more")
    text = "\n".join(lines)
    if len(text) > cap:
        text = text[: cap - 40].rsplit("\n", 1)[0] + "\n- report truncated at the cap"
    return text


_HEADING2 = re.compile(r"^##\s+(.*?)\s*$")


def _key(text: str) -> str:
    """A comparison key that survives the rewrites a stored document may apply —
    an escaped character, a dash variant, changed spacing. Matching the raw
    string instead would miss the stored heading and append a duplicate section
    on every run."""
    return re.sub(r"[^a-z0-9]", "", text.lower())


class SpliceError(Exception):
    """The report section is not in a shape this tool can replace safely."""


def splice_section(content: str, heading: str, body: str, end: str = REPORT_END) -> str:
    """``content`` with the section from the level-2 ``heading`` through the
    ``end`` line replaced, or the section appended when the heading is absent.

    Bounded by an explicit end line rather than by the next heading, so text
    appended below the report — a note, a paragraph, a deeper heading — is never
    swallowed. Refuses rather than guesses when the heading repeats or its end
    line is missing."""
    lines = content.split("\n")
    starts = [
        i
        for i, ln in enumerate(lines)
        if (m := _HEADING2.match(ln)) and _key(m.group(1)) == _key(heading)
    ]
    if len(starts) > 1:
        raise SpliceError(f"the report heading appears {len(starts)} times")
    section = [f"## {heading}", "", body, "", end]
    if not starts:
        return content.rstrip("\n") + "\n\n" + "\n".join(section) + "\n"
    start = starts[0]
    stop = None
    for j in range(start + 1, len(lines)):
        # The end line matches near-exactly (escapes and spacing only), unlike the
        # heading: a loose key would let a report item that merely reads like the
        # marker end the section early.
        if lines[j].replace("\\", "").strip() == end:
            stop = j
            break
        if _HEADING2.match(lines[j]):
            # The next section began first: the end line is gone, and searching
            # past here would delete whatever lies between.
            break
    if stop is None:
        raise SpliceError("the report heading has no end line; not replacing")
    new = lines[:start] + section + lines[stop + 1 :]
    return "\n".join(new).rstrip("\n") + "\n"


_DOC_UPDATE = """
mutation Update($id: String!, $content: String!) {
  documentUpdate(id: $id, input: { content: $content }) { success }
}
"""


def write_report(report: str) -> None:
    """Replace the report heading in the Planning document. Read and write sit
    back to back to keep the window for a concurrent edit as small as possible;
    Linear offers no conditional write to close it outright."""
    key = linear_api.env_var("LINEAR_API_KEY")
    doc_id = linear_api.env_var("LINEAR_PLANNING_DOC_ID")
    _, content = planning_doc.fetch(key, doc_id)
    updated = splice_section(content, REPORT_HEADING, defang(report))
    data = linear_api.post(key, _DOC_UPDATE, {"id": doc_id, "content": updated})
    if not (data.get("documentUpdate") or {}).get("success"):
        raise linear_api.LinearApiError("documentUpdate reported no success")


# --- stamp -----------------------------------------------------------------


def stamp_path(base: Path) -> Path:
    return base / ".claude" / STAMP_NAME


def write_stamp(
    base: Path, now: datetime, armed: bool, parked: dict | None = None
) -> None:
    stamp = {"last_run": now.isoformat(), "armed": armed}
    if parked is not None:
        stamp["parked"] = parked
    stamp_path(base).write_text(json.dumps(stamp) + "\n", encoding="utf-8")


def previous_parked(path: Path) -> dict | None:
    """The parked counts the last stamped pass saw, or None before the first."""
    try:
        parked = json.loads(path.read_text("utf-8")).get("parked")
    except (OSError, ValueError, AttributeError):
        return None
    return parked if isinstance(parked, dict) else None


def previous_run(path: Path) -> str | None:
    """When the last stamped pass ran — where the next digest starts."""
    try:
        last = json.loads(path.read_text("utf-8")).get("last_run")
    except (OSError, ValueError, AttributeError):
        return None
    return last if isinstance(last, str) and last else None


def fold_stamp_path(base: Path) -> Path:
    return base / ".claude" / FOLD_STAMP_NAME


def write_fold_stamp(base: Path, now: datetime, batches: int, folds: int) -> None:
    """The launcher's record that the day's lever fold ran — beside the pass's
    own stamp, read by the scheduler's bootstrap instead of a fold report."""
    stamp = {"ran_at": now.isoformat(), "batches": batches, "folds": folds}
    fold_stamp_path(base).write_text(json.dumps(stamp) + "\n", encoding="utf-8")


def fold_today(path: Path, now: datetime) -> str:
    """The bootstrap's one line: ``fold ran at HH:MM, N batches`` (local time,
    with ``, M folded into open issues`` when M > 0), or ``no fold today`` for
    a missing or unreadable stamp, or one not from today's local date."""
    try:
        stamp = json.loads(path.read_text("utf-8"))
        ran = datetime.fromisoformat(stamp["ran_at"]).astimezone()
        batches = int(stamp["batches"])
        folds = int(stamp.get("folds", 0))
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        return "no fold today"
    if ran.date() != now.astimezone().date():
        return "no fold today"
    line = f"fold ran at {ran:%H:%M}, {batches} batches"
    return line + (f", {folds} folded into open issues" if folds else "")


def ran_today(path: Path, now: datetime) -> dict:
    """Whether the stamp's local calendar date is ``now``'s."""
    try:
        last = datetime.fromisoformat(json.loads(path.read_text("utf-8"))["last_run"])
    except (OSError, ValueError, KeyError, TypeError):
        return {"ran_today": False, "last_run": None}
    return {
        "ran_today": last.astimezone().date() == now.astimezone().date(),
        "last_run": last.isoformat(),
    }


# --- CLI -------------------------------------------------------------------


def to_stderr(line: str) -> None:
    # Flushed per line: a terminal is watching, not a log file.
    print(line, file=sys.stderr, flush=True)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="upkeep.py", description=__doc__.split("\n")[0]
    )
    sub = parser.add_subparsers(dest="cmd", required=True)
    p_run = sub.add_parser("run", help="run the upkeep pass")
    p_run.add_argument(
        "--json", action="store_true", help="print the structured result"
    )
    p_run.add_argument(
        "--no-doc", action="store_true", help="skip the Planning document write"
    )
    p_run.add_argument(
        "--if-stale",
        action="store_true",
        help="skip the pass when today's run is already stamped",
    )
    sub.add_parser("ran-today", help="did a pass run today? (no network read)")
    p_fold = sub.add_parser(
        "fold-stamp", help="record that the launcher's lever fold ran (no network)"
    )
    p_fold.add_argument("--batches", type=int, required=True, help="batches filed")
    p_fold.add_argument(
        "--folds", type=int, default=0, help="parts folded into open issues"
    )
    sub.add_parser("fold-today", help="the scheduler's fold line (no network read)")
    p_prune = sub.add_parser(
        "prune",
        help="only the cleanup steps (PR set, board, prune, notifications); "
        "a dry run unless --apply",
    )
    p_prune.add_argument(
        "--apply", action="store_true", help="remove and dismiss for real"
    )
    p_prune.add_argument(
        "--json", action="store_true", help="print the structured result"
    )
    args = parser.parse_args(argv[1:])

    base, trees, why = check_base(real_run, os.getcwd())
    if base is None:
        print(f"upkeep: refused — {why}", file=sys.stderr)
        return 2
    now = datetime.now(timezone.utc)

    if args.cmd == "ran-today":
        print(json.dumps(ran_today(stamp_path(base), now)))
        return 0
    if args.cmd == "fold-stamp":
        write_fold_stamp(base, now, args.batches, args.folds)
        print(fold_today(fold_stamp_path(base), now))
        return 0
    if args.cmd == "fold-today":
        print(fold_today(fold_stamp_path(base), now))
        return 0
    if args.cmd == "run" and args.if_stale:
        seen = ran_today(stamp_path(base), now)
        if seen["ran_today"]:
            print(f"upkeep: already ran today ({seen['last_run']}); skipped")
            return 0

    if args.cmd == "prune":
        # Armed by the flag, not the environment: the caller is the operator,
        # whose `--apply` is the approval. The gates that make that safe are
        # the board rule (completed or canceled, no open PR) and the pruner's
        # live-lock hold, not a prompt: a closed name's dirty or unpushed work
        # no longer holds it. No document write and no stamp — this is not the
        # day's pass.
        ctx = Ctx(
            base=base,
            armed=args.apply,
            now=now,
            dismiss=args.apply,
            lookup=linear_lookup,
            progress=to_stderr,
        )
        ctx.data["trees"] = trees
        result = run_pass(ctx, PRUNE_STEPS)
        print(json.dumps(result, indent=2) if args.json else render(result))
        return 0

    ctx = Ctx(
        base=base,
        armed=os.environ.get(ARM_ENV) == "1",
        now=now,
        dismiss=True,
        lookup=linear_lookup,
        parked=linear_parked,
        progress=to_stderr,
    )
    ctx.data["trees"] = trees
    ctx.data["previous_parked"] = previous_parked(stamp_path(base))
    ctx.data["previous_run"] = previous_run(stamp_path(base))
    result = run_pass(ctx)
    report = render(result)
    if not args.no_doc:
        try:
            write_report(report)
            result["doc"] = "written"
        except Exception as e:  # noqa: BLE001 — stdout is the fallback
            result["doc"] = f"not written: {first_line(str(e))}"
        report += f"\n- planning document: {result['doc']}"
    # Print before stamping: stdout is the fallback when the document write
    # failed, so nothing after this point may be able to lose it.
    print(json.dumps(result, indent=2) if args.json else report)
    # Stamp only a pass whose report reached the document. "Ran today" gates the
    # day's pass, so a preview or a failed write must not answer it.
    if result.get("doc") == "written":
        try:
            # A failed count carries the last good one forward, so one Linear
            # blip does not cost the next pass its direction.
            parked = result.get("parked") or ctx.data["previous_parked"]
            write_stamp(base, now, ctx.armed, parked)
        except OSError as e:
            print(f"upkeep: run stamp not written: {e}", file=sys.stderr)
    return 0


def main() -> int:
    return run(sys.argv)


if __name__ == "__main__":
    sys.exit(main())
