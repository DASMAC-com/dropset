#!/usr/bin/env python3
"""Unit tests for ``session_metrics.py``.

Stdlib ``unittest`` only — run via the repo's ``make tools-tests`` (no pytest
dependency). These mirror the former Rust ``model.rs`` tests and add coverage
for the hardening-candidate detector.
"""

from __future__ import annotations

import json
import unittest

import session_metrics as sm


def assistant(usage: str, tool_uses: str) -> str:
    """A compact assistant record with one usage block and any tool_use items."""
    return json.dumps(
        {
            "type": "assistant",
            "message": {
                "role": "assistant",
                "usage": json.loads(usage),
                "content": json.loads(f"[{tool_uses}]") if tool_uses else [],
            },
        }
    )


def assistant_with_id(msg_id: str, usage: str, tool_uses: str) -> str:
    """An assistant record carrying a logical message id, to model the
    one-record-per-content-block split that repeats the same usage.
    """
    return json.dumps(
        {
            "type": "assistant",
            "message": {
                "role": "assistant",
                "id": msg_id,
                "usage": json.loads(usage),
                "content": json.loads(f"[{tool_uses}]") if tool_uses else [],
            },
        }
    )


def tool_use(tid: str, name: str, input_json: str) -> str:
    return json.dumps(
        {"type": "tool_use", "id": tid, "name": name, "input": json.loads(input_json)}
    )


def tool_result(tid: str, content_json: str) -> str:
    return json.dumps(
        {
            "type": "user",
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": tid,
                        "content": json.loads(content_json),
                    }
                ],
            },
        }
    )


class TokenAccounting(unittest.TestCase):
    def test_sums_usage_across_turns(self):
        agg = sm.SessionAggregator()
        agg.ingest_main_line(
            assistant(
                '{"input_tokens":100,"output_tokens":50,'
                '"cache_creation_input_tokens":200,"cache_read_input_tokens":700}',
                "",
            )
        )
        agg.ingest_main_line(
            assistant(
                '{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":300}',
                "",
            )
        )
        report = agg.finish()
        totals = report["totals"]
        self.assertEqual(totals.input, 110)
        self.assertEqual(totals.output, 55)
        self.assertEqual(totals.cache_creation, 200)
        self.assertEqual(totals.cache_read, 1000)
        self.assertEqual(totals.turns, 2)
        self.assertAlmostEqual(report["cache_hit_rate"], 1000.0 / 1310.0, places=9)

    def test_attributes_results_to_their_tool(self):
        agg = sm.SessionAggregator()
        agg.ingest_main_line(
            assistant(
                '{"output_tokens":1}',
                "{},{}".format(
                    tool_use("t1", "Read", '{"file_path":"/a/b/fixture.rs"}'),
                    tool_use("t2", "Bash", '{"command":"cargo test -p dropset-tui"}'),
                ),
            )
        )
        # 40-byte result for the Read, 4-byte for the Bash.
        agg.ingest_main_line(
            tool_result("t1", '"0123456789012345678901234567890123456789"')
        )
        agg.ingest_main_line(tool_result("t2", '"abcd"'))
        report = agg.finish()

        read = next(t for t in report["tools"] if t.name == "Read")
        self.assertEqual(read.calls, 1)
        self.assertEqual(read.result_bytes, 40)
        self.assertEqual(report["top_sinks"][0].name, "Read")
        self.assertEqual(report["top_sinks"][0].bytes, 40)
        self.assertEqual(report["top_sinks"][0].label, "/a/b/fixture.rs")
        self.assertEqual(report["top_sinks"][1].name, "Bash")
        self.assertEqual(report["top_sinks"][1].label, "cargo test -p dropset-tui")

    def test_usage_counted_once_per_message_id(self):
        agg = sm.SessionAggregator()
        usage = '{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":700}'
        for _ in range(3):
            agg.ingest_main_line(assistant_with_id("msg_aaa", usage, ""))
        agg.ingest_main_line(assistant_with_id("msg_bbb", '{"output_tokens":5}', ""))
        report = agg.finish()
        totals = report["totals"]
        self.assertEqual(totals.input, 100)  # once, not 3×
        self.assertEqual(totals.output, 55)
        self.assertEqual(totals.cache_read, 700)
        self.assertEqual(totals.turns, 2)  # two logical messages, not four records

    def test_subagent_usage_counted_once_per_message_id(self):
        agg = sm.SessionAggregator()
        usage = '{"input_tokens":5000,"output_tokens":300}'
        for _ in range(4):
            agg.ingest_subagent_line("agent-x", assistant_with_id("msg_sub", usage, ""))
        report = agg.finish()
        self.assertEqual(len(report["subagents"]), 1)
        self.assertEqual(report["subagents"][0].turns, 1)
        self.assertEqual(report["subagents"][0].input, 5000)
        self.assertEqual(report["subagents"][0].output, 300)

    def test_unmatched_result_falls_back_to_unknown(self):
        agg = sm.SessionAggregator()
        agg.ingest_main_line(tool_result("orphan", '"data"'))
        report = agg.finish()
        self.assertEqual(report["tools"][0].name, "unknown")
        self.assertEqual(report["tools"][0].calls, 1)

    def test_array_content_result_is_measured_by_serialization(self):
        agg = sm.SessionAggregator()
        agg.ingest_main_line(
            assistant(
                '{"output_tokens":1}', tool_use("t1", "Grep", '{"pattern":"foo"}')
            )
        )
        agg.ingest_main_line(tool_result("t1", '[{"type":"text","text":"a result"}]'))
        report = agg.finish()
        grep = next(t for t in report["tools"] if t.name == "Grep")
        self.assertGreater(grep.result_bytes, 0)

    def test_subagent_usage_rolls_up_per_agent(self):
        agg = sm.SessionAggregator()
        agg.ingest_subagent_line(
            "agent-explore",
            assistant(
                '{"input_tokens":5000,"output_tokens":300,"cache_read_input_tokens":1000}',
                "",
            ),
        )
        agg.ingest_subagent_line(
            "agent-explore",
            assistant('{"input_tokens":100,"output_tokens":20}', ""),
        )
        report = agg.finish()
        self.assertEqual(len(report["subagents"]), 1)
        a = report["subagents"][0]
        self.assertEqual(a.agent, "agent-explore")
        self.assertEqual(a.turns, 2)
        self.assertEqual(a.input, 5100)
        self.assertEqual(a.output, 320)
        self.assertEqual(a.cache_read, 1000)
        self.assertEqual(report["tools"], [])

    def test_malformed_lines_are_counted_not_fatal(self):
        agg = sm.SessionAggregator()
        agg.ingest_main_line("{not valid json")
        agg.ingest_main_line("")
        agg.ingest_main_line(assistant('{"output_tokens":7}', ""))
        report = agg.finish()
        self.assertEqual(report["parse_errors"], 1)  # blank line skipped, not an error
        self.assertEqual(report["totals"].output, 7)

    def test_non_message_records_are_ignored(self):
        agg = sm.SessionAggregator()
        agg.ingest_main_line('{"type":"summary","summary":"a title"}')
        agg.ingest_main_line(
            '{"type":"attachment","attachment":{"type":"skill_listing"}}'
        )
        report = agg.finish()
        self.assertEqual(report["totals"].turns, 0)
        self.assertEqual(report["parse_errors"], 0)


class BashSignatures(unittest.TestCase):
    def test_signature_keeps_stable_head(self):
        self.assertEqual(
            sm.bash_signature("git worktree list --porcelain"), "git worktree list"
        )
        self.assertEqual(
            sm.bash_signature("git branch -m worktree-eng-1 eng-1"), "git branch"
        )
        self.assertEqual(sm.bash_signature("printenv LINEAR_TEAM_ID"), "printenv")
        self.assertEqual(sm.bash_signature("gh pr checks 183"), "gh pr checks")

    def test_signature_strips_env_assignments(self):
        self.assertEqual(sm.bash_signature("FOO=bar git status --short"), "git status")

    def test_signature_collapses_path_args(self):
        # `-C` flag is skipped; the path arg ends the stable head.
        self.assertEqual(
            sm.bash_signature("git -C /Users/a/repo pull --ff-only"), "git pull"
        )

    def test_signature_unwraps_the_quiet_runner(self):
        """It used to fuse the wrapper with its payload — `python3 make lint` —
        which is unreadable and risks nominating the wrapper as a candidate."""
        self.assertEqual(
            sm.bash_signature("python3 .claude/tools/run_quiet.py -- make lint"),
            "make lint",
        )
        self.assertEqual(
            sm.bash_signature(
                "python3 .claude/tools/run_quiet.py -- pnpm --dir frontend build"
            ),
            "pnpm frontend build",
        )

    def test_unwrapping_survives_a_leading_env_assignment(self):
        self.assertEqual(
            sm.bash_signature(
                "FOO=bar python3 .claude/tools/run_quiet.py -- make test"
            ),
            "make test",
        )

    def test_the_runner_as_a_bare_argument_is_not_unwrapped(self):
        """No `--` separator means it is not a wrapper invocation — staging or
        reading the tool must keep its own shape."""
        self.assertEqual(
            sm.bash_signature("git add .claude/tools/run_quiet.py"), "git add"
        )

    def test_an_unwrapped_command_still_normalizes(self):
        self.assertEqual(sm.bash_signature("make lint"), "make lint")

    def test_a_repo_tool_is_named_by_its_script_not_by_python3(self):
        """Otherwise every repo tool collapses into one `python3` shape, which
        three sessions then reported as their top hardening candidate."""
        self.assertEqual(
            sm.bash_signature("python3 .claude/tools/search_source.py 'pat'"),
            "search_source.py",
        )
        self.assertEqual(
            sm.bash_signature("python3 .claude/tools/init_pr_branch.py --tag eng-1"),
            "init_pr_branch.py",
        )

    def test_distinct_repo_tools_get_distinct_shapes(self):
        a = sm.bash_signature("python3 .claude/tools/allowlist.py cruft")
        b = sm.bash_signature("python3 .claude/tools/board_batch.py list")
        self.assertNotEqual(a, b)

    def test_run_quiet_still_unwraps_rather_than_naming_itself(self):
        """The unwrap must win over the script-naming rule."""
        self.assertEqual(
            sm.bash_signature("python3 .claude/tools/run_quiet.py -- make lint"),
            "make lint",
        )

    def test_a_module_invocation_is_named_by_its_module(self):
        """`-m` used to leave the head a bare `python3`, so every `-m` call
        collapsed into one shape — the same defect the script case fixes."""
        sig = sm.bash_signature("python3 -m unittest discover -s tests")
        self.assertTrue(sig.startswith("unittest discover"), sig)
        self.assertNotIn("python3", sig)
        # Two different modules must not share a shape.
        self.assertNotEqual(sig, sm.bash_signature("python3 -m pytest -q"))

    def test_a_point_release_interpreter_is_recognized(self):
        """An enumerated set silently fell back to collapsing on a new
        release."""
        self.assertEqual(
            sm.bash_signature("python3.13 .claude/tools/board_batch.py list"),
            "board_batch.py list",
        )

    def test_a_non_python_script_is_named_by_its_script_too(self):
        self.assertEqual(
            sm.bash_signature("node decks/scripts/fetch-remote-assets.mjs"),
            "fetch-remote-assets.mjs",
        )

    def test_a_bare_interpreter_with_no_script_is_unchanged(self):
        self.assertEqual(sm.bash_signature("python3 foo bar.py"), "python3 foo")


class RepoToolExclusion(unittest.TestCase):
    def test_repo_tool_shapes_are_recognized(self):
        self.assertTrue(sm.is_repo_tool_shape("search_source.py"))
        self.assertFalse(sm.is_repo_tool_shape("make lint"))
        self.assertFalse(sm.is_repo_tool_shape("git worktree list"))
        self.assertFalse(sm.is_repo_tool_shape(""))

    def test_a_non_python_script_is_still_a_hardening_candidate(self):
        """Naming and exclusion are different questions: a build script is
        named by its script, but it is not one of the repo's Python
        skill-tools, so it must stay eligible."""
        self.assertFalse(sm.is_repo_tool_shape("fetch-remote-assets.mjs"))

    def test_a_repo_tool_is_kept_out_of_the_hardening_table(self):
        """It is already the hardened form; nominating it crowds out real
        candidates. A non-tool repeat in the same run still lands."""
        agg = sm.SessionAggregator()
        commands = [
            "python3 .claude/tools/search_source.py 'pat'",
            "python3 .claude/tools/search_source.py 'other'",
            "make lint",
            "make lint",
        ]
        for i, cmd in enumerate(commands):
            agg.ingest_main_line(
                assistant(
                    '{"output_tokens":1}',
                    tool_use(f"b{i}", "Bash", json.dumps({"command": cmd})),
                )
            )
        report = agg.finish()
        sigs = {c.signature for c in report["hardening_candidates"]}
        self.assertNotIn("search_source.py", sigs)
        self.assertIn("make lint", sigs)

    def test_deterministic_classification(self):
        self.assertTrue(sm.is_deterministic_shape("git worktree list"))
        self.assertTrue(sm.is_deterministic_shape("git branch"))
        self.assertTrue(sm.is_deterministic_shape("printenv"))
        self.assertFalse(sm.is_deterministic_shape("git pull"))
        self.assertFalse(sm.is_deterministic_shape("cargo test"))
        self.assertFalse(sm.is_deterministic_shape("make lint"))

    def test_hardening_candidates_surface_repeats(self):
        agg = sm.SessionAggregator()
        # `git worktree list` runs twice (a deterministic repeat); `make lint`
        # runs twice (a repeat, but not deterministic string logic); `git status`
        # runs once (below the recurrence threshold). Each genuine call gets its
        # own tool_use id.
        commands = [
            "git worktree list --porcelain",
            "git worktree list --porcelain",
            "make lint",
            "make lint",
            "git status --short",
        ]
        for i, cmd in enumerate(commands):
            agg.ingest_main_line(
                assistant(
                    '{"output_tokens":1}',
                    tool_use(f"b{i}", "Bash", json.dumps({"command": cmd})),
                )
            )
        report = agg.finish()
        by_signature = {c.signature: c for c in report["hardening_candidates"]}
        self.assertIn("git worktree list", by_signature)
        self.assertEqual(by_signature["git worktree list"].count, 2)
        self.assertTrue(by_signature["git worktree list"].deterministic)
        self.assertIn("make lint", by_signature)
        self.assertFalse(by_signature["make lint"].deterministic)
        self.assertNotIn("git status", by_signature)  # only ran once

    def test_bash_signature_deduped_by_tool_use_id(self):
        # A split assistant message re-walks the same tool_use block across its
        # content-block records; the signature must be counted once per id, not
        # once per record (else a single call inflates the hardening count).
        agg = sm.SessionAggregator()
        line = assistant(
            '{"output_tokens":1}',
            tool_use(
                "b1", "Bash", json.dumps({"command": "git worktree list --porcelain"})
            ),
        )
        agg.ingest_main_line(line)
        agg.ingest_main_line(line)  # same tool_use id seen again (the split)
        report = agg.finish()
        by_signature = {c.signature: c for c in report["hardening_candidates"]}
        # Counted once → below the recurrence threshold → not surfaced.
        self.assertNotIn("git worktree list", by_signature)


class HardeningRanking(unittest.TestCase):
    """The table is ranked by result size and labels which cost each shape is.

    Ranking by call count misled five consecutive sessions: `grep` topped it every
    time while being negligible by size, and hoisting a shape converts many small
    calls into a few large ones — so a count-ranked table flags the fix as the new
    problem.
    """

    def setUp(self):
        """Pin the allowlist so these tests never read machine-local settings.

        `_load_allowlist` reads the operator's shared `settings.local.json`, so
        left live the coverage label would depend on whose machine ran the
        suite — `printenv LINEAR_TEAM_ID` really is covered here, which turned
        two of these assertions red for a reason that has nothing to do with
        the code. Default to "covers nothing"; the tests that care about the
        wiring set it themselves.
        """
        real = sm._load_allowlist
        self.addCleanup(lambda: setattr(sm, "_load_allowlist", real))
        sm._load_allowlist = lambda: []

    def _run(self, calls):
        """Ingest ``[(command, result_text)]`` as real call/result pairs."""
        agg = sm.SessionAggregator()
        for i, (cmd, result) in enumerate(calls):
            agg.ingest_main_line(
                assistant(
                    '{"output_tokens":1}',
                    tool_use(f"b{i}", "Bash", json.dumps({"command": cmd})),
                )
            )
            agg.ingest_main_line(tool_result(f"b{i}", json.dumps(result)))
        return agg.finish()

    def test_ranked_by_result_bytes_not_count(self):
        """Many tiny greps must not outrank one big diff."""
        grep_cmd = "grep -rn needle src"
        diff_cmd = "git diff main"
        calls = [(grep_cmd, "hit\n")] * 20
        calls += [(diff_cmd, "x" * 5000)] * 2
        report = self._run(calls)
        signatures = [c.signature for c in report["hardening_candidates"]]

        grep_sig = sm.bash_signature(grep_cmd)
        diff_sig = sm.bash_signature(diff_cmd)
        # The grep ran 10x as often; the diff returned far more, and wins.
        self.assertEqual(signatures[0], diff_sig)
        self.assertLess(
            signatures.index(diff_sig), signatures.index(grep_sig), signatures
        )
        by_sig = {c.signature: c for c in report["hardening_candidates"]}
        self.assertEqual(by_sig[grep_sig].count, 20)
        self.assertEqual(by_sig[diff_sig].count, 2)

    def test_result_bytes_are_attributed_to_the_shape(self):
        report = self._run([("git diff main", "x" * 100)] * 3)
        candidate = report["hardening_candidates"][0]
        self.assertEqual(candidate.count, 3)
        # json.dumps adds the surrounding quotes, hence >= rather than ==.
        self.assertGreaterEqual(candidate.result_bytes, 300)
        self.assertGreaterEqual(candidate.avg_bytes(), 100)

    def test_a_big_result_is_a_context_cost(self):
        report = self._run([("git diff main", "x" * 5000)] * 2)
        self.assertEqual(report["hardening_candidates"][0].cost_kind(), "context")

    def test_a_quiet_runner_command_is_a_wall_clock_cost(self):
        """`make lint` through run_quiet returns one summary line — the cost is
        time, not tokens, and three sessions had to say so by hand."""
        cmd = "python3 .claude/tools/run_quiet.py -- make lint"
        report = self._run([(cmd, "OK make lint (exit 0, 1392 lines)")] * 6)
        candidate = report["hardening_candidates"][0]
        self.assertTrue(candidate.via_run_quiet)
        self.assertEqual(candidate.cost_kind(), "wall-clock")

    def test_a_quiet_runner_command_with_a_big_tail_names_the_failures(self):
        """A failing run_quiet call prints a real tail, so bytes win over the
        wrapper — but the label has to say *why*, or the reader concludes the
        wrapper is broken. One session filed a defect against `run_quiet.py` on
        exactly that misreading; the classification was right all along."""
        cmd = "python3 .claude/tools/run_quiet.py -- make lint"
        report = self._run([(cmd, "x" * 4000)] * 2)
        candidate = report["hardening_candidates"][0]
        self.assertTrue(candidate.via_run_quiet)
        self.assertEqual(candidate.cost_kind(), "context (failures)")

    def test_an_unwrapped_big_result_stays_plain_context(self):
        """The two must stay distinguishable: unwrapped means the lever is to
        wrap it, wrapped means the lever is to fail less."""
        report = self._run([("make lint", "x" * 4000)] * 2)
        candidate = report["hardening_candidates"][0]
        self.assertFalse(candidate.via_run_quiet)
        self.assertEqual(candidate.cost_kind(), "context")

    def test_a_cheap_fast_repeat_is_prompt_churn(self):
        """`printenv` costs neither tokens nor time — it is worth a tool because
        each variant re-prompts, so mislabeling it wall-clock would be wrong."""
        report = self._run([("printenv LINEAR_TEAM_ID", "abc\n")] * 4)
        candidate = report["hardening_candidates"][0]
        self.assertFalse(candidate.via_run_quiet)
        self.assertEqual(candidate.cost_kind(), "prompt-churn")

    def test_an_allowlisted_cheap_repeat_is_not_called_churn(self):
        """The heuristic cannot see a prompt: many cheap, slightly-varying calls
        look identical whether they re-prompted or not. One filed lever argued
        for a whole new tool on that basis before its own author checked
        coverage and withdrew the reasoning."""
        report = self._run([("printenv LINEAR_TEAM_ID", "abc\n")] * 4)
        candidate = report["hardening_candidates"][0]
        candidate.allowlisted = True
        self.assertEqual(candidate.cost_kind(), "covered (no churn)")

    def test_coverage_never_masks_a_real_token_sink(self):
        """Coverage is checked last, so it can only downgrade a churn claim. A
        covered shape returning large results is still `context`, because the
        cost there is the bytes and has nothing to do with prompting."""
        report = self._run([("make lint", "x" * 4000)] * 2)
        candidate = report["hardening_candidates"][0]
        candidate.allowlisted = True
        self.assertEqual(candidate.cost_kind(), "context")

    def test_an_unresolvable_allowlist_reports_what_it_reported_before(self):
        """The safe direction: no allowlist means no downgrade."""
        report = self._run([("printenv LINEAR_TEAM_ID", "abc\n")] * 4)
        self.assertFalse(report["hardening_candidates"][0].allowlisted)
        self.assertEqual(report["hardening_candidates"][0].cost_kind(), "prompt-churn")

    def test_a_covered_shape_is_marked_from_the_real_allowlist(self):
        """The wiring itself — without this the label ships inert, which is
        exactly what two independent review lenses caught."""
        sm._load_allowlist = lambda: ["Bash(printenv:*)"]
        report = self._run([("printenv LINEAR_TEAM_ID", "abc\n")] * 4)
        candidate = report["hardening_candidates"][0]
        self.assertTrue(candidate.allowlisted)
        self.assertEqual(candidate.cost_kind(), "covered (no churn)")

    def test_an_uncovered_shape_is_still_churn_with_a_live_allowlist(self):
        """A non-empty allowlist must not blanket-mark everything."""
        sm._load_allowlist = lambda: ["Bash(git status:*)"]
        report = self._run([("printenv LINEAR_TEAM_ID", "abc\n")] * 4)
        self.assertEqual(report["hardening_candidates"][0].cost_kind(), "prompt-churn")

    def test_run_quiet_flag_is_sticky_across_a_shape(self):
        """One unwrapped call among wrapped ones is a slip, not a
        re-classification — the bytes check still catches a real payload.

        Unwrapping is what makes this test meaningful: a wrapped and a bare
        invocation of the same command now share one signature, so the sticky
        flag has a shape to be sticky *across*. Before, they grouped under
        `python3 make lint` and `make lint` separately.
        """
        quiet = "python3 .claude/tools/run_quiet.py -- make lint"
        report = self._run([(quiet, "OK\n"), ("make lint", "OK\n")])
        candidate = report["hardening_candidates"][0]
        self.assertEqual(candidate.signature, "make lint")
        self.assertEqual(candidate.count, 2)
        self.assertTrue(candidate.via_run_quiet)

    def test_determinism_is_still_reported(self):
        """It says how *portable* a shape is — a separate question from cost."""
        report = self._run([("git worktree list --porcelain", "ok\n")] * 2)
        self.assertTrue(report["hardening_candidates"][0].deterministic)

    def test_a_shape_with_no_result_still_counts(self):
        """A call whose result never arrived (an interrupted turn) must not vanish
        from the table — it just contributes no bytes."""
        agg = sm.SessionAggregator()
        for i in range(2):
            agg.ingest_main_line(
                assistant(
                    '{"output_tokens":1}',
                    tool_use(f"b{i}", "Bash", json.dumps({"command": "make lint"})),
                )
            )
        report = agg.finish()
        candidate = report["hardening_candidates"][0]
        self.assertEqual(candidate.count, 2)
        self.assertEqual(candidate.result_bytes, 0)
        self.assertEqual(candidate.avg_bytes(), 0)


class Rendering(unittest.TestCase):
    def test_markdown_smoke(self):
        agg = sm.SessionAggregator()
        agg.ingest_main_line(
            assistant(
                '{"input_tokens":10,"output_tokens":5}',
                tool_use("t1", "Read", '{"file_path":"/a.rs"}'),
            )
        )
        agg.ingest_main_line(tool_result("t1", '"some content here"'))
        md = sm.to_markdown(agg.finish(), "abcd1234")
        self.assertIn("## Session metrics — abcd1234", md)
        self.assertIn("Costliest tools", md)

    def test_json_smoke(self):
        agg = sm.SessionAggregator()
        agg.ingest_main_line(assistant('{"output_tokens":7}', ""))
        parsed = json.loads(sm.to_json(agg.finish()))
        self.assertEqual(parsed["totals"]["output"], 7)
        self.assertIn("hardening_candidates", parsed)

    def test_markdown_renders_the_cost_column_and_its_legend(self):
        agg = sm.SessionAggregator()
        cmd = "python3 .claude/tools/run_quiet.py -- make lint"
        for i in range(2):
            agg.ingest_main_line(
                assistant(
                    '{"output_tokens":1}',
                    tool_use(f"b{i}", "Bash", json.dumps({"command": cmd})),
                )
            )
            agg.ingest_main_line(tool_result(f"b{i}", '"OK"'))
        md = sm.to_markdown(agg.finish(), "abcd1234")
        self.assertIn("by result size", md)
        self.assertIn("wall-clock", md)
        # the legend explains the three kinds, so a reader needn't guess
        self.assertIn("hardening it buys latency, not tokens", md)

    def test_json_carries_the_cost_fields(self):
        agg = sm.SessionAggregator()
        for i in range(2):
            agg.ingest_main_line(
                assistant(
                    '{"output_tokens":1}',
                    tool_use(f"b{i}", "Bash", json.dumps({"command": "git diff main"})),
                )
            )
            agg.ingest_main_line(tool_result(f"b{i}", json.dumps("x" * 5000)))
        parsed = json.loads(sm.to_json(agg.finish()))
        candidate = parsed["hardening_candidates"][0]
        self.assertEqual(candidate["cost_kind"], "context")
        self.assertGreater(candidate["result_bytes"], 5000)
        self.assertFalse(candidate["via_run_quiet"])


if __name__ == "__main__":
    unittest.main()
