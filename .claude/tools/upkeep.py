#!/usr/bin/env python3
# cspell:word defang
"""The model-free morning upkeep pass — the mechanical half of ``housekeeping``,
composed from the committed helpers that skill drove one call at a time.

Usage (from the base repo root; a worktree is refused):

    python3 .claude/tools/upkeep.py run [--json] [--no-doc]
    python3 .claude/tools/upkeep.py ran-today

``run`` executes ten steps in order, each under its own timeout and each
**best-effort**: a failure is a report line, never an abort of later steps.
The one exception is step 1 — running from anywhere but the base checkout
refuses the whole pass, since every later step assumes it.

 1. confirm the base repo root;
 2. fast-forward main (on failure, continue on the checked-out commit);
 3. upgrade the Claude Code CLI cask;
 4. read the terminal PR set (merged AND closed — a PR closed early leaves its
    branch and notifications behind too), the open set, and the PR
    notifications;
 5. intersect with the board: a worktree, local branch or notification is
    cleanup-eligible when its issue's status TYPE is completed or canceled and
    no PR for its branch is open. PR-merged alone is never enough — a merged
    In Review session is still a live conversation the fleet launcher resumes.
    A canceled issue with an open PR is flagged; a branch whose issue cannot be
    resolved is reported; Linear unreachable cleans NOTHING;
 6. prune the eligible worktrees and stray local branches (``prune_worktrees``,
    whose unpushed-work gate still applies);
 7. dismiss the eligible PRs' notifications, one thread at a time (never
    mark-all);
 8. convention-reference check, hook-wiring check, allowlist cruft, and the
    monthly refresh gate — mining recent transcripts when due and stamping the
    marker whatever the yield, writing no allow-rule;
 9. the memory scan gate, then the memory audit when due;
10. the transcript-purge dry-run manifest.

**Arming.** Steps 6 and 7 run as dry runs and touch nothing unless
``DS_UPKEEP_ARMED=1`` is exported from the untracked runtime config. The
open-PR guard holds even when armed, and the purge never arms — its apply stays
an operator approval at ``plan``'s gate.

**Report.** One heading of the Planning document — ``REPORT_HEADING`` — is
replaced wholesale each run, bounded to ``REPORT_CHAR_CAP``. The same report
prints to stdout (the standalone interface, and the fallback when Linear is
unreachable); ``--json`` prints the structured twin instead. A local stamp beside
the allowlist refresh marker answers ``ran-today`` with no network read.

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
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

import board_batch
import linear_api
import planning_doc
import prune_conversations
import prune_worktrees

TOOLS = Path(__file__).resolve().parent
REPO = "DASMAC-com/dropset"
ARM_ENV = "DS_UPKEEP_ARMED"
REPORT_HEADING = "Upkeep report — machine-written, latest run only"
STAMP_NAME = ".upkeep-last-run.json"

# Roughly 1.5k tokens. The section is replaced every run, so the cap bounds what
# every planning bootstrap pays to read it, not just this run's write.
REPORT_CHAR_CAP = 6000
LIST_CAP = 6

TIMEOUT_GIT = 120
TIMEOUT_BREW = 60
TIMEOUT_GH = 60
TIMEOUT_TOOL = 120

# How many recent transcripts the refresh gate mines, and how many notification
# PRs outside the listed sets are resolved one by one before giving up.
MINE_SESSIONS = 8
MAX_NOTIFICATION_LOOKUPS = 20

NOTIFICATIONS_PATH = "/notifications?all=true&per_page=50"

RESOLVED_TYPES = ("completed", "canceled")
BRANCH_RE = re.compile(r"^eng-(\d+)$")
PR_URL_RE = re.compile(r"/repos/DASMAC-com/dropset/pulls/(\d+)$")

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
    run: Runner = real_run
    # numbers -> {number: status type}; raises when Linear is unreachable.
    lookup: Callable[[list[int]], dict[int, str]] | None = None
    steps: list[dict] = field(default_factory=list)
    data: dict = field(default_factory=dict)

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
                "30",
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
                "100",
            ],
        )
    except (RuntimeError, ValueError) as e:
        ctx.record("prs", f"PR list failed ({e}); nothing is cleanup-eligible", False)
        return
    prs = list(closed or []) + list(opened or [])
    by_number = {int(p["number"]): p for p in prs}

    notifications: list[tuple[str, int]] = []
    try:
        # `all=true`: the default lists UNREAD threads only, and a thread read
        # but never marked done still sits in the inbox.
        raw = _gh_json(ctx, ["api", NOTIFICATIONS_PATH])
        for n in raw or []:
            m = PR_URL_RE.search(((n.get("subject") or {}).get("url")) or "")
            if m:
                notifications.append((str(n["id"]), int(m.group(1))))
    except (RuntimeError, ValueError, KeyError) as e:
        ctx.record("prs", f"notification list failed ({e})", False)

    unknown = sorted({num for _, num in notifications if num not in by_number})
    for num in unknown[:MAX_NOTIFICATION_LOOKUPS]:
        try:
            pr = _gh_json(
                ctx,
                ["pr", "view", str(num), "--repo", REPO, "--json", fields],
            )
            by_number[num] = pr
            prs.append(pr)
        except (RuntimeError, ValueError):
            pass

    ctx.data["prs"] = prs
    ctx.data["pr_by_number"] = by_number
    ctx.data["notifications"] = notifications
    ctx.data["open_branches"] = sorted(
        p["headRefName"] for p in opened or [] if p.get("headRefName")
    )
    ctx.record(
        "prs",
        f"{len(closed or [])} terminal PR(s), {len(opened or [])} open, "
        f"{len(notifications)} PR notification(s)",
    )


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
    in_trees = {t["branch"] for t in ctx.data.get("trees", []) if t.get("branch")}
    by_number = ctx.data["pr_by_number"]
    notified = {
        by_number[num]["headRefName"]
        for _, num in ctx.data["notifications"]
        if num in by_number and by_number[num].get("headRefName")
    }
    candidates = (local | in_trees | notified) - {"main"}
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
    eligible, flags = eligible_branches(
        candidates, pr_state_by_branch(ctx.data["prs"]), status
    )
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
        merged_file = f.name
    try:
        args = ["--merged-file", merged_file] + ([] if ctx.armed else ["--dry-run"])
        rc, out, err = ctx.tool("prune_worktrees.py", *args)
    finally:
        os.unlink(merged_file)
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
    )


def step_notifications(ctx: Ctx) -> None:
    eligible = ctx.data.get("eligible", set())
    by_number = ctx.data.get("pr_by_number", {})
    threads = [
        tid
        for tid, num in ctx.data.get("notifications", [])
        if by_number.get(num, {}).get("headRefName") in eligible
        and by_number[num].get("state") != "OPEN"
    ]
    if not ctx.armed:
        ctx.record("notifications", f"would dismiss {len(threads)} notification(s)")
        return
    failed = 0
    for tid in threads:
        # DELETE on a thread is GitHub's "mark as done" — read-only marking would
        # leave it in the inbox.
        rc, _, _ = ctx.run(
            ["gh", "api", "-X", "DELETE", f"/notifications/threads/{tid}"],
            TIMEOUT_GH,
            None,
        )
        failed += rc != 0
    ctx.record(
        "notifications",
        f"dismissed {len(threads) - failed} notification(s)"
        + (f", {failed} failed" if failed else ""),
        ok=not failed,
    )


def _json_tool(ctx: Ctx, step: str, name: str, *args: str, ok_rcs=(0, 1)):
    rc, out, err = ctx.tool(name, *args)
    if rc not in ok_rcs:
        ctx.record(step, f"{name} failed: {first_line(err)}", False)
        return None
    try:
        return json.loads(out)
    except ValueError:
        ctx.record(step, f"{name} printed no JSON", False)
        return None


def recent_sessions(base: Path, limit: int = MINE_SESSIONS) -> list[str]:
    """Ids of the newest transcripts under the base checkout and its worktrees."""
    root = prune_conversations.projects_root()
    prefix = prune_conversations.slugify(base)
    files = [
        p for d in root.glob(prefix + "*") if d.is_dir() for p in d.glob("*.jsonl")
    ]
    files.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    return [p.stem for p in files[:limit]]


def mine_refresh_candidates(ctx: Ctx) -> list[str]:
    """Uncovered repeated Bash shapes across recent sessions, most frequent
    first. Read-only: the candidates go to the report, never into settings."""
    counts: dict[str, int] = {}
    for sid in recent_sessions(ctx.base):
        rc, out, _ = ctx.tool("session_metrics.py", "--session-id", sid, "--json")
        if rc != 0:
            continue
        try:
            report = json.loads(out)
        except ValueError:
            continue
        for c in report.get("hardening_candidates", []):
            if c.get("cost_kind") == "prompt-churn":
                counts[c["signature"]] = counts.get(c["signature"], 0) + c["count"]
    ranked = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))
    return [f"{sig} ({n} calls)" for sig, n in ranked]


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
    candidates = mine_refresh_candidates(ctx)
    rc, _, err = ctx.tool("allowlist.py", "refresh-record", "--added", "0")
    stamped = "stamped" if rc == 0 else f"stamp failed: {first_line(err)}"
    ctx.record(
        "allowlist-refresh",
        f"refresh due; {len(candidates)} uncovered shape(s) mined; {stamped}",
        ok=rc == 0,
        candidates=candidates,
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
    ctx.tool("memory_scan_gate.py", "record", str(memory_dir))
    ctx.record(
        "memory", f"memory audit ran: {len(findings)} finding(s)", findings=findings
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
            args += ["--completed-slug", prune_conversations.slugify(Path(r["path"]))]
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
    ctx.record("purge", f"purge dry-run: {total} to free (apply is plan's gate)")


STEPS = (
    ("pull", step_pull),
    ("cli", step_cli),
    ("prs", step_prs),
    ("board", step_board),
    ("prune", step_prune),
    ("notifications", step_notifications),
    ("checks", step_checks),
    ("memory", step_memory),
    ("purge", step_purge),
)


def run_pass(ctx: Ctx) -> dict:
    for name, fn in STEPS:
        try:
            fn(ctx)
        except Exception as e:  # noqa: BLE001 — best-effort: a crash is a line
            ctx.record(
                name, f"step crashed: {type(e).__name__}: {first_line(str(e))}", False
            )
    return {
        "ran_at": ctx.now.isoformat(),
        "armed": ctx.armed,
        "steps": ctx.steps,
    }


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


LIST_KEYS = ("flags", "skipped", "dangling", "inert", "cruft", "candidates", "findings")


def render(result: dict, cap: int = REPORT_CHAR_CAP) -> str:
    mode = "armed" if result["armed"] else "dry run (disarmed)"
    lines = [f"Run {result['ran_at']}, {mode}.", ""]
    for s in result["steps"]:
        mark = "" if s["ok"] else " (failed)"
        lines.append(f"- {s['step']}{mark}: {s['line']}")
        for key in LIST_KEYS:
            items = s.get(key) or []
            for item in items[:LIST_CAP]:
                lines.append(f"  - {item}")
            if len(items) > LIST_CAP:
                lines.append(f"  - and {len(items) - LIST_CAP} more")
    text = "\n".join(lines)
    if len(text) > cap:
        text = text[: cap - 40].rsplit("\n", 1)[0] + "\n- report truncated at the cap"
    return text


_HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*$")


def splice_section(content: str, heading: str, body: str) -> str:
    """``content`` with ``heading``'s section replaced by ``body`` — through the
    next heading of the same or a higher level — or appended as a level-2
    section when absent."""
    lines = content.split("\n")
    start = level = None
    for i, ln in enumerate(lines):
        m = _HEADING.match(ln)
        if m and m.group(2) == heading:
            start, level = i, len(m.group(1))
            break
    if start is None:
        return content.rstrip("\n") + f"\n\n## {heading}\n\n{body}\n"
    end = len(lines)
    for j in range(start + 1, len(lines)):
        m = _HEADING.match(lines[j])
        if m and len(m.group(1)) <= level:
            end = j
            break
    tail = lines[end:]
    new = lines[: start + 1] + ["", body, ""] + tail
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


def write_stamp(base: Path, now: datetime, armed: bool) -> None:
    stamp_path(base).write_text(
        json.dumps({"last_run": now.isoformat(), "armed": armed}) + "\n",
        encoding="utf-8",
    )


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
    sub.add_parser("ran-today", help="did a pass run today? (no network read)")
    args = parser.parse_args(argv[1:])

    base, trees, why = check_base(real_run, os.getcwd())
    if base is None:
        print(f"upkeep: refused — {why}", file=sys.stderr)
        return 2
    now = datetime.now(timezone.utc)

    if args.cmd == "ran-today":
        print(json.dumps(ran_today(stamp_path(base), now)))
        return 0

    ctx = Ctx(
        base=base,
        armed=os.environ.get(ARM_ENV) == "1",
        now=now,
        lookup=linear_lookup,
    )
    ctx.data["trees"] = trees
    result = run_pass(ctx)
    report = render(result)
    if not args.no_doc:
        try:
            write_report(report)
            result["doc"] = "written"
        except Exception as e:  # noqa: BLE001 — stdout is the fallback
            result["doc"] = f"not written: {first_line(str(e))}"
        report += f"\n- planning document: {result['doc']}"
    write_stamp(base, now, ctx.armed)
    print(json.dumps(result, indent=2) if args.json else report)
    return 0


def main() -> int:
    return run(sys.argv)


if __name__ == "__main__":
    sys.exit(main())
