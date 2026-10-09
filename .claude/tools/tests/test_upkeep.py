# cspell:word defang
# cspell:word basenames
"""Stdlib ``unittest`` tests for the model-free upkeep pass.

Run via the repo's ``make tools-tests``. Every external call goes through an
injected runner and lookup, so nothing here touches git, GitHub or Linear.
"""

import contextlib
import io
import json
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path

import planning_doc
import prune_conversations
import upkeep
from upkeep import (
    REPORT_END,
    Ctx,
    SpliceError,
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

    def test_splice_appends_with_its_end_line_when_absent(self):
        out = splice_section("# Planning\n\nintro\n", "Upkeep", "new", end="End.")
        self.assertTrue(out.endswith("## Upkeep\n\nnew\n\nEnd.\n"))
        self.assertIn("intro", out)

    def test_splice_keeps_text_appended_below_the_report(self):
        # A note another session appended after the report, with no level-2
        # heading of its own, used to be swallowed by the next run.
        doc = (
            "# Planning\n\nintro\n\n## Upkeep\n\nold\n\nEnd.\n\n"
            "a later note\n\n### a deeper heading\n\nmore\n"
        )
        out = splice_section(doc, "Upkeep", "new", end="End.")
        self.assertNotIn("old", out)
        self.assertIn("## Upkeep\n\nnew\n\nEnd.\n\na later note", out)
        self.assertIn("### a deeper heading\n\nmore", out)

    def test_splice_is_idempotent_and_tolerates_a_rewritten_heading(self):
        once = splice_section("# Planning\n", "Upkeep report — latest", "a", end="End.")
        # A stored copy may escape or swap the dash; it must still match.
        stored = once.replace("—", "\\-")
        twice = splice_section(stored, "Upkeep report — latest", "b", end="End.")
        self.assertEqual(twice.count("## Upkeep report"), 1)
        self.assertIn("b\n\nEnd.", twice)
        self.assertNotIn("\na\n", twice)

    def test_splice_refuses_a_missing_end_line_or_a_repeated_heading(self):
        with self.assertRaises(SpliceError):
            splice_section("## Upkeep\n\nold\n\n## Next\n", "Upkeep", "new", end="End.")
        with self.assertRaises(SpliceError):
            splice_section(
                "## Upkeep\n\nEnd.\n\n## Upkeep\n", "Upkeep", "x", end="End."
            )

    def test_a_lost_end_line_refuses_rather_than_reaching_past_the_next_section(self):
        doc = "## Upkeep\n\nold\n\n## Next\n\nkeep\n\nquoting End. in a note\nEnd.\n"
        with self.assertRaises(SpliceError):
            splice_section(doc, "Upkeep", "new", end="End.")

    def test_an_item_that_reads_like_the_end_line_does_not_end_the_section(self):
        body = "- step: x\n  - End of upkeep report"
        once = splice_section("# Planning\n", "Upkeep", body, end=REPORT_END)
        twice = splice_section(once, "Upkeep", "- step: y", end=REPORT_END)
        self.assertEqual(twice.count(REPORT_END), 1)
        self.assertNotIn("step: x", twice)

    def test_a_level_one_heading_is_not_taken_as_the_report(self):
        doc = "# Upkeep\n\nthe whole plan\n\n## Next\n\nkeep\n"
        out = splice_section(doc, "Upkeep", "new", end="End.")
        self.assertIn("the whole plan", out)
        self.assertIn("## Next\n\nkeep", out)

    def test_render_collapses_multi_line_items(self):
        result = {
            "ran_at": "t",
            "armed": False,
            "steps": [
                {"step": "prune", "ok": True, "line": "x", "skipped": ["a\n## Next\nb"]}
            ],
        }
        self.assertNotIn("\n## Next", render(result))

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

    OPEN = [{"number": 13, "headRefName": "eng-13", "state": "OPEN"}]

    def __init__(
        self,
        notifications=(),
        prune_out=None,
        open_prs=None,
        closed_fails=False,
        refresh_due=False,
        metrics_out=None,
    ):
        self.calls = []
        self.notifications = list(notifications)
        self.open_prs = self.OPEN if open_prs is None else open_prs
        self.closed_fails = closed_fails
        self.refresh_due = refresh_due
        self.metrics_out = metrics_out
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
            if "closed" in cmd and self.closed_fails:
                return 1, "", "HTTP 502"
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
            return 0, json.dumps(self.open_prs), ""
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
            return 0, json.dumps({"due": self.refresh_due, "reason": "r"}), ""
        if tool == "allowlist.py" and "refresh-record" in cmd:
            return 0, "{}", ""
        if tool == "session_metrics.py" and self.metrics_out is not None:
            return 0, json.dumps(self.metrics_out), ""
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
        # Through the real parser: a slug of an absolute path starts with "-",
        # which argparse rejects as a separate value.
        args = prune_conversations.build_parser().parse_args(purge[2:])
        path = Path(f"{BASE}/.claude/worktrees/eng-10")
        self.assertEqual(args.completed_slug, [prune_conversations.slugify(path)])
        self.assertEqual(args.protected_branch, ["eng-13"])
        self.assertFalse(args.apply)

    def test_a_failed_pr_list_makes_nothing_eligible(self):
        run = FakeRun(closed_fails=True, notifications=[("t10", 10)])
        ctx = make_ctx(run, armed=True)
        run_pass(ctx)
        self.assertEqual(ctx.data["eligible"], set())
        self.assertEqual(tool_calls(run, "prune_worktrees.py"), [])
        self.assertFalse(any(c[:3] == ["gh", "api", "-X"] for c in run.calls))

    def test_a_full_open_pr_page_fails_closed(self):
        full = [
            {"number": 1000 + i, "headRefName": f"eng-{1000 + i}", "state": "OPEN"}
            for i in range(upkeep.OPEN_PR_LIMIT)
        ]
        ctx = make_ctx(FakeRun(open_prs=full), armed=True)
        run_pass(ctx)
        self.assertEqual(ctx.data["eligible"], set())
        prs = next(s for s in ctx.steps if s["step"] == "prs")
        self.assertIn("limit", prs["line"])

    def test_a_completed_issue_with_an_open_pr_is_kept_by_the_whole_pass(self):
        # eng-10's issue is completed and it has a local trace, but it also has an
        # OPEN PR: neither its worktree nor its notification may be touched.
        open_prs = [{"number": 20, "headRefName": "eng-10", "state": "OPEN"}]
        run = FakeRun(open_prs=open_prs, notifications=[("t20", 20), ("t10", 10)])
        ctx = make_ctx(run, armed=True)
        run_pass(ctx)
        self.assertNotIn("eng-10", ctx.data["eligible"])
        merged_file_calls = tool_calls(run, "prune_worktrees.py")
        self.assertTrue(merged_file_calls)  # eng-12 is still eligible
        deletes = [c[-1] for c in run.calls if c[:3] == ["gh", "api", "-X"]]
        self.assertEqual(deletes, [])

    def test_unmatched_names_reach_the_report(self):
        out = {
            "removed": [],
            "branches_removed": [],
            "skipped": [],
            "unmatched": ["eng-12"],
            "dry_run": True,
        }
        ctx = make_ctx(FakeRun(prune_out=out))
        text = render(run_pass(ctx))
        self.assertIn("eng-12: matches no worktree or branch", text)

    def test_a_due_refresh_mines_reports_and_stamps_without_adding(self):
        metrics = {
            "hardening_candidates": [
                {"signature": "make lint", "count": 4, "cost_kind": "prompt-churn"},
                {
                    "signature": "git status",
                    "count": 9,
                    "cost_kind": "covered (no churn)",
                },
            ]
        }
        run = FakeRun(refresh_due=True, metrics_out=metrics)
        saved = upkeep.recent_sessions
        upkeep.recent_sessions = lambda base: ["s1", "s2"]
        try:
            ctx = make_ctx(run)
            run_pass(ctx)
        finally:
            upkeep.recent_sessions = saved
        step = next(s for s in ctx.steps if s["step"] == "allowlist-refresh")
        self.assertTrue(step["ok"])
        self.assertEqual(step["candidates"], ["make lint (8 calls)"])
        self.assertIn("from 2 session(s); stamped", step["line"])
        allow = tool_calls(run, "allowlist.py")
        self.assertIn([*allow[-1][:2], "refresh-record", "--added", "0"], allow)
        self.assertFalse(any("add" in c[2:3] for c in allow))

    def test_a_refresh_that_mined_nothing_is_not_stamped(self):
        run = FakeRun(refresh_due=True)
        saved = upkeep.recent_sessions
        upkeep.recent_sessions = lambda base: []
        try:
            ctx = make_ctx(run)
            run_pass(ctx)
        finally:
            upkeep.recent_sessions = saved
        step = next(s for s in ctx.steps if s["step"] == "allowlist-refresh")
        self.assertFalse(step["ok"])
        self.assertFalse(
            any("refresh-record" in c for c in tool_calls(run, "allowlist.py"))
        )


class WriteReportTests(unittest.TestCase):
    def test_the_report_is_made_safe_spliced_and_written(self):
        posted = {}
        doc = "# Planning\n\nintro\n"
        saved = (planning_doc.fetch, upkeep.linear_api.post, upkeep.linear_api.env_var)
        planning_doc.fetch = lambda key, doc_id: ("Planning", doc)

        def post(key, query, variables, **kw):
            posted.update(variables)
            return {"documentUpdate": {"success": True}}

        upkeep.linear_api.post = post
        upkeep.linear_api.env_var = lambda name, **kw: "x"
        try:
            upkeep.write_report("- allowlist: settings.local.json flagged `Bash(rm:*)`")
        finally:
            planning_doc.fetch, upkeep.linear_api.post, upkeep.linear_api.env_var = (
                saved
            )
        content = posted["content"]
        self.assertTrue(content.startswith("# Planning\n\nintro\n"))
        self.assertIn(REPORT_END, content)
        self.assertNotIn(".json", content)
        self.assertNotIn("`", content)
        self.assertNotIn("*", content)


class CliTests(unittest.TestCase):
    def setUp(self):
        self.saved = (upkeep.check_base, upkeep.run_pass, upkeep.write_report)

    def tearDown(self):
        upkeep.check_base, upkeep.run_pass, upkeep.write_report = self.saved

    def test_a_refused_base_exits_two(self):
        upkeep.check_base = lambda run, cwd: (None, [], "not the base")
        with contextlib.redirect_stderr(io.StringIO()) as err:
            self.assertEqual(upkeep.run(["upkeep.py", "run"]), 2)
        self.assertIn("refused", err.getvalue())

    def test_a_written_report_stamps_the_run(self):
        with tempfile.TemporaryDirectory() as d:
            base = Path(d)
            (base / ".claude").mkdir()
            upkeep.check_base = lambda run, cwd: (base, [], "")
            upkeep.run_pass = lambda ctx: {"ran_at": "t", "armed": False, "steps": []}
            upkeep.write_report = lambda report: None
            with contextlib.redirect_stdout(io.StringIO()):
                upkeep.run(["upkeep.py", "run"])
            self.assertTrue(upkeep.stamp_path(base).exists())

    def test_no_doc_json_prints_the_result_without_stamping(self):
        with tempfile.TemporaryDirectory() as d:
            base = Path(d)
            (base / ".claude").mkdir()
            upkeep.check_base = lambda run, cwd: (base, [], "")
            upkeep.run_pass = lambda ctx: {"ran_at": "t", "armed": False, "steps": []}
            upkeep.write_report = lambda report: self.fail("doc write not skipped")
            with contextlib.redirect_stdout(io.StringIO()) as out:
                self.assertEqual(
                    upkeep.run(["upkeep.py", "run", "--no-doc", "--json"]), 0
                )
            self.assertEqual(json.loads(out.getvalue())["ran_at"], "t")
            self.assertFalse(upkeep.stamp_path(base).exists())

    def test_a_failed_doc_write_is_reported_on_stdout(self):
        with tempfile.TemporaryDirectory() as d:
            base = Path(d)
            (base / ".claude").mkdir()
            upkeep.check_base = lambda run, cwd: (base, [], "")
            upkeep.run_pass = lambda ctx: {"ran_at": "t", "armed": False, "steps": []}

            def boom(report):
                raise RuntimeError("Linear down")

            upkeep.write_report = boom
            with contextlib.redirect_stdout(io.StringIO()) as out:
                upkeep.run(["upkeep.py", "run"])
            self.assertIn("planning document: not written: Linear down", out.getvalue())
            self.assertFalse(upkeep.stamp_path(base).exists())


if __name__ == "__main__":
    unittest.main()
