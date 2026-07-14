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
        # Deterministic candidates rank ahead of non-deterministic ones.
        self.assertTrue(report["hardening_candidates"][0].deterministic)

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


if __name__ == "__main__":
    unittest.main()
