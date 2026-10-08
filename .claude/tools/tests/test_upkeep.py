# cspell:word defang
# cspell:word basenames
"""Stdlib ``unittest`` tests for the model-free upkeep pass.

Run via the repo's ``make tools-tests``. Every external call goes through an
injected runner and lookup, so nothing here touches git, GitHub or Linear.
"""

import json
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path

from upkeep import (
    Ctx,
    check_base,
    classify,
    defang,
    eligible_branches,
    pr_state_by_branch,
    ran_today,
    render,
    run_pass,
    splice_section,
)

BASE = "/repo/dropset"
PORCELAIN = f"""\
worktree {BASE}
HEAD abc
branch refs/heads/main

worktree {BASE}/.claude/worktrees/eng-10
HEAD def
branch refs/heads/eng-10

worktree {BASE}/.claude/worktrees/eng-11
HEAD 012
branch refs/heads/eng-11
"""


class ClassifyTests(unittest.TestCase):
    def test_status_type_gates_and_merged_alone_is_not_enough(self):
        self.assertEqual(classify("MERGED", "completed"), (True, None))
        self.assertEqual(classify("MERGED", "started"), (False, None))
        self.assertEqual(classify("CLOSED", "canceled"), (True, None))
        self.assertEqual(classify(None, "completed"), (True, None))
        self.assertEqual(classify(None, "unstarted"), (False, None))

    def test_an_open_pr_is_never_eligible(self):
        self.assertEqual(classify("OPEN", "completed"), (False, None))
        self.assertEqual(
            classify("OPEN", "canceled"), (False, "canceled issue with an open PR")
        )

    def test_an_unresolvable_issue_fails_closed(self):
        self.assertEqual(classify("MERGED", None), (False, "issue unresolvable"))

    def test_open_beats_merged_beats_closed_per_branch(self):
        states = pr_state_by_branch(
            [
                {"headRefName": "eng-1", "state": "CLOSED"},
                {"headRefName": "eng-1", "state": "OPEN"},
                {"headRefName": "eng-1", "state": "MERGED"},
                {"headRefName": "eng-2", "state": "CLOSED"},
                {"headRefName": "eng-2", "state": "MERGED"},
            ]
        )
        self.assertEqual(states, {"eng-1": "OPEN", "eng-2": "MERGED"})

    def test_eligible_branches_reports_flags(self):
        eligible, flags = eligible_branches(
            {"eng-1", "eng-2", "eng-3", "scratch"},
            {"eng-1": "MERGED", "eng-2": "OPEN"},
            {1: "completed", 2: "canceled"},
        )
        self.assertEqual(eligible, {"eng-1"})
        self.assertIn("eng-2: canceled issue with an open PR", flags)
        self.assertIn("eng-3: issue unresolvable", flags)
        self.assertIn("scratch: issue unresolvable", flags)


class ReportTests(unittest.TestCase):
    def test_defang_neutralizes_basenames_emphasis_and_code_spans(self):
        out = defang("settings.local.json has `Bash(git status:*)` and MEMORY.md")
        self.assertNotIn(".json", out)
        self.assertNotIn(".md", out)
        self.assertNotIn("*", out)
        self.assertNotIn("`", out)
        self.assertIn("settings", out)

    def test_render_caps_each_list_and_the_whole(self):
        result = {
            "ran_at": "2026-10-08T00:00:00+00:00",
            "armed": False,
            "steps": [
                {
                    "step": "board",
                    "ok": True,
                    "line": "x",
                    "flags": [f"f{i}" for i in range(20)],
                }
            ],
        }
        text = render(result)
        self.assertIn("dry run (disarmed)", text)
        self.assertIn("and 14 more", text)
        self.assertNotIn("f19", text)
        many = dict(
            result,
            steps=[
                {"step": f"s{i}", "ok": False, "line": "y" * 80} for i in range(200)
            ],
        )
        capped = render(many, cap=2000)
        self.assertLessEqual(len(capped), 2000)
        self.assertTrue(capped.endswith("report truncated at the cap"))

    def test_splice_replaces_only_the_named_section(self):
        doc = "# Planning\n\nintro\n\n## Upkeep\n\nold\n\n### Detail\n\nold too\n\n## Next\n\nkeep\n"
        out = splice_section(doc, "Upkeep", "new")
        self.assertIn("## Upkeep\n\nnew\n\n## Next\n\nkeep", out)
        self.assertNotIn("old", out)
        self.assertIn("intro", out)

    def test_splice_appends_when_the_heading_is_absent(self):
        out = splice_section("# Planning\n\nintro\n", "Upkeep", "new")
        self.assertTrue(out.endswith("## Upkeep\n\nnew\n"))
        self.assertIn("intro", out)

    def test_ran_today_reads_the_local_stamp(self):
        now = datetime(2026, 10, 8, 12, tzinfo=timezone.utc)
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "stamp.json"
            self.assertFalse(ran_today(path, now)["ran_today"])
            path.write_text(json.dumps({"last_run": now.isoformat()}))
            self.assertTrue(ran_today(path, now)["ran_today"])
            old = now - timedelta(days=2)
            path.write_text(json.dumps({"last_run": old.isoformat()}))
            self.assertFalse(ran_today(path, now)["ran_today"])


class FakeRun:
    """Answers each external command; records every call."""

    def __init__(self, notifications=(), prune_out=None):
        self.calls = []
        self.notifications = list(notifications)
        self.prune_out = prune_out or {
            "removed": [],
            "branches_removed": [],
            "skipped": [],
            "dry_run": True,
        }

    def __call__(self, cmd, timeout, cwd=None):
        self.calls.append(cmd)
        if cmd[:2] == ["git", "worktree"]:
            return 0, PORCELAIN, ""
        if cmd[:2] == ["git", "rev-parse"]:
            return 0, BASE + "\n", ""
        if cmd[:2] == ["git", "for-each-ref"]:
            return 0, "main\neng-10\neng-11\neng-12\n", ""
        if cmd[:2] == ["git", "pull"]:
            return 0, "Already up to date.\n", ""
        if cmd[0] == "brew":
            return 0, "", ""
        if cmd[:3] == ["gh", "pr", "list"]:
            if "closed" in cmd:
                return (
                    0,
                    json.dumps(
                        [
                            {"number": 10, "headRefName": "eng-10", "state": "MERGED"},
                            {"number": 11, "headRefName": "eng-11", "state": "MERGED"},
                            {"number": 12, "headRefName": "eng-12", "state": "CLOSED"},
                        ]
                    ),
                    "",
                )
            return (
                0,
                json.dumps([{"number": 13, "headRefName": "eng-13", "state": "OPEN"}]),
                "",
            )
        if cmd[:2] == ["gh", "api"] and cmd[2].startswith("/notifications?all=true"):
            return (
                0,
                json.dumps(
                    [
                        {
                            "id": tid,
                            "subject": {
                                "url": f"https://api.github.com/repos/DASMAC-com/dropset/pulls/{n}"
                            },
                        }
                        for tid, n in self.notifications
                    ]
                ),
                "",
            )
        if cmd[:3] == ["gh", "api", "-X"]:
            return 0, "", ""
        tool = Path(cmd[1]).name if len(cmd) > 1 else ""
        if tool == "prune_worktrees.py":
            return 0, json.dumps(self.prune_out), ""
        if tool == "allowlist.py" and "refresh-due" in cmd:
            return 0, json.dumps({"due": False, "reason": "recent"}), ""
        if tool == "memory_scan_gate.py":
            return 0, json.dumps({"scan": False, "reason": "unchanged"}), ""
        return 1, "", "unexpected"


def make_ctx(run, armed=False, lookup=None):
    ctx = Ctx(
        base=Path(BASE),
        armed=armed,
        now=datetime(2026, 10, 8, tzinfo=timezone.utc),
        run=run,
        lookup=lookup
        or (lambda nums: {10: "completed", 11: "started", 12: "canceled"}),
    )
    from prune_worktrees import parse_worktrees

    ctx.data["trees"] = parse_worktrees(PORCELAIN)
    return ctx


def tool_calls(run, name):
    return [c for c in run.calls if len(c) > 1 and Path(c[1]).name == name]


class PassTests(unittest.TestCase):
    def test_check_base_refuses_a_worktree(self):
        def run(cmd, timeout, cwd=None):
            if cmd[:2] == ["git", "rev-parse"]:
                return 0, f"{BASE}/.claude/worktrees/eng-10\n", ""
            return 0, PORCELAIN, ""

        base, _, why = check_base(run, "/x")
        self.assertIsNone(base)
        self.assertIn("not a worktree", why)

    def test_disarmed_pass_dry_runs_the_eligible_set_only(self):
        run = FakeRun(notifications=[("t10", 10), ("t11", 11), ("t13", 13)])
        ctx = make_ctx(run)
        run_pass(ctx)
        self.assertEqual(ctx.data["eligible"], {"eng-10", "eng-12"})
        prune_call = tool_calls(run, "prune_worktrees.py")[0]
        self.assertIn("--dry-run", prune_call)
        self.assertFalse(any(c[:3] == ["gh", "api", "-X"] for c in run.calls))
        line = next(s for s in ctx.steps if s["step"] == "notifications")["line"]
        self.assertEqual(line, "would dismiss 1 notification(s)")

    def test_armed_pass_dismisses_only_eligible_threads(self):
        run = FakeRun(notifications=[("t10", 10), ("t11", 11), ("t13", 13)])
        ctx = make_ctx(run, armed=True)
        run_pass(ctx)
        self.assertNotIn("--dry-run", tool_calls(run, "prune_worktrees.py")[0])
        deletes = [c for c in run.calls if c[:3] == ["gh", "api", "-X"]]
        self.assertEqual(
            deletes, [["gh", "api", "-X", "DELETE", "/notifications/threads/t10"]]
        )

    def test_linear_unreachable_cleans_nothing(self):
        def down(nums):
            raise RuntimeError("connection refused")

        run = FakeRun(notifications=[("t10", 10)])
        ctx = make_ctx(run, armed=True, lookup=down)
        run_pass(ctx)
        self.assertEqual(ctx.data["eligible"], set())
        self.assertEqual(tool_calls(run, "prune_worktrees.py"), [])
        self.assertFalse(any(c[:3] == ["gh", "api", "-X"] for c in run.calls))
        board = next(s for s in ctx.steps if s["step"] == "board")
        self.assertFalse(board["ok"])
        self.assertIn("cleaned nothing", board["line"])

    def test_a_crashing_step_does_not_stop_later_ones(self):
        run = FakeRun()
        # A malformed lookup answer breaks the board step outside its own guard.
        ctx = make_ctx(run, lookup=lambda nums: 42)
        run_pass(ctx)
        board = next(s for s in ctx.steps if s["step"] == "board")
        self.assertIn("step crashed", board["line"])
        names = [s["step"] for s in ctx.steps]
        self.assertIn("purge", names)
        self.assertIn("memory", names)

    def test_cruft_and_memory_lines_count_findings_not_totals(self):
        base_run = FakeRun()

        def run(cmd, timeout, cwd=None):
            tool = Path(cmd[1]).name if len(cmd) > 1 else ""
            if tool == "allowlist.py" and "cruft" in cmd:
                flagged = [{"rule": "Bash(rm:*)", "category": "dangerous"}]
                return 0, json.dumps({"count": 376, "flagged": flagged}), ""
            if tool == "memory_scan_gate.py" and "check" in cmd:
                return 0, json.dumps({"scan": True, "reason": "changed"}), ""
            if tool == "memory_audit.py":
                return (
                    0,
                    "dangling-path: a — gone\nmemory-audit | 9 memories | 1 finding(s)\n",
                    "",
                )
            return base_run(cmd, timeout, cwd)

        ctx = make_ctx(run)
        run_pass(ctx)
        lines = {s["step"]: s["line"] for s in ctx.steps}
        self.assertEqual(lines["allowlist"], "1 cruft entry(ies) of 376 rule(s)")
        self.assertEqual(lines["memory"], "memory audit ran: 1 finding(s)")

    def test_only_an_actual_removal_frees_slugs_in_the_purge(self):
        removed = {
            "removed": [
                {"path": f"{BASE}/.claude/worktrees/eng-10", "branch": "eng-10"}
            ],
            "branches_removed": [],
            "skipped": [],
        }
        dry = FakeRun(prune_out=dict(removed, dry_run=True))
        run_pass(make_ctx(dry))
        self.assertNotIn(
            "--completed-slug", tool_calls(dry, "prune_conversations.py")[0]
        )
        armed = FakeRun(prune_out=dict(removed, dry_run=False))
        run_pass(make_ctx(armed, armed=True))
        purge = tool_calls(armed, "prune_conversations.py")[0]
        self.assertIn("--completed-slug", purge)
        self.assertIn("--protected-branch", purge)
        self.assertNotIn("--apply", purge)


if __name__ == "__main__":
    unittest.main()
