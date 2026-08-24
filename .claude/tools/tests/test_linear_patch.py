#!/usr/bin/env python3
"""Unit tests for linear_patch.py.

Two properties carry the tool and are tested hardest:

* **zero echo** — no stored body may appear in anything printed. That is the
  entire reason this exists beside the MCP write path, whose per-call echo
  compounds on an accumulator issue (ten saves, ~41.1k, to add ~2k).
* **atomicity** — an ops sequence either applies whole or not at all. A
  half-applied patch on an issue body is worse than a refused one, and the
  caller cannot see the body to repair it.

Nothing here touches the network.
"""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import linear_patch as lp  # noqa: E402
from linear_patch import LinearPatchError  # noqa: E402

BODY = "# Title\n\nAlpha line\n\nBeta line\n\nGamma line\n"


class ApplyOpsTests(unittest.TestCase):
    def test_append_and_prepend(self):
        self.assertEqual(
            lp.apply_ops("mid", [{"op": "append", "text": "-end"}]), "mid-end"
        )
        self.assertEqual(
            lp.apply_ops("mid", [{"op": "prepend", "text": "start-"}]), "start-mid"
        )

    def test_insert_before_and_after(self):
        out = lp.apply_ops(
            BODY, [{"op": "insert_before", "anchor": "Beta", "text": "X"}]
        )
        self.assertIn("XBeta", out)
        out = lp.apply_ops(
            BODY, [{"op": "insert_after", "anchor": "Beta", "text": "X"}]
        )
        self.assertIn("BetaX", out)

    def test_replace_and_replace_range(self):
        out = lp.apply_ops(
            BODY, [{"op": "replace", "anchor": "Beta line", "text": "Bee"}]
        )
        self.assertIn("Bee", out)
        self.assertNotIn("Beta line", out)

        out = lp.apply_ops(
            BODY,
            [
                {
                    "op": "replace_range",
                    "start": "Alpha",
                    "end": "Gamma line",
                    "text": "ONLY",
                }
            ],
        )
        self.assertIn("ONLY", out)
        self.assertNotIn("Beta", out)

    def test_ops_apply_in_order(self):
        out = lp.apply_ops(
            "base",
            [{"op": "append", "text": "-one"}, {"op": "append", "text": "-two"}],
        )
        self.assertEqual(out, "base-one-two")

    def test_a_later_op_can_anchor_on_an_earlier_ops_output(self):
        out = lp.apply_ops(
            "base",
            [
                {"op": "append", "text": "\nMARKER"},
                {"op": "insert_after", "anchor": "MARKER", "text": "!"},
            ],
        )
        self.assertEqual(out, "base\nMARKER!")

    def test_a_missing_anchor_raises_before_anything_is_written(self):
        with self.assertRaises(LinearPatchError) as caught:
            lp.apply_ops(BODY, [{"op": "replace", "anchor": "nope", "text": "x"}])
        self.assertIn("not found", str(caught.exception))

    def test_an_ambiguous_anchor_is_refused_rather_than_resolved(self):
        # Picking the first match would silently edit a different part of the
        # issue than the caller meant, and they cannot see the body from here.
        with self.assertRaises(LinearPatchError) as caught:
            lp.apply_ops(
                "dup\ndup\n", [{"op": "replace", "anchor": "dup", "text": "x"}]
            )
        self.assertIn("exactly once", str(caught.exception))

    def test_an_anchor_naming_an_issue_is_refused_with_the_reason(self):
        with self.assertRaises(LinearPatchError) as caught:
            lp.apply_ops(
                BODY, [{"op": "replace", "anchor": "see ENG-942", "text": "x"}]
            )
        self.assertIn("mention", str(caught.exception))

    def test_a_failing_op_aborts_the_whole_sequence(self):
        # The first op would have succeeded; nothing may be returned.
        with self.assertRaises(LinearPatchError):
            lp.apply_ops(
                BODY,
                [
                    {"op": "append", "text": "fine"},
                    {"op": "replace", "anchor": "nope", "text": "x"},
                ],
            )

    def test_end_before_start_is_refused(self):
        with self.assertRaises(LinearPatchError) as caught:
            lp.apply_ops(
                BODY,
                [
                    {
                        "op": "replace_range",
                        "start": "Gamma",
                        "end": "Alpha",
                        "text": "x",
                    }
                ],
            )
        self.assertIn("before", str(caught.exception))

    def test_unknown_op_empty_list_and_over_cap_are_all_refused(self):
        with self.assertRaises(LinearPatchError):
            lp.apply_ops(BODY, [{"op": "obliterate", "text": "x"}])
        with self.assertRaises(LinearPatchError):
            lp.apply_ops(BODY, [])
        with self.assertRaises(LinearPatchError) as caught:
            lp.apply_ops(BODY, [{"op": "append", "text": "x"}] * (lp.MAX_OPS + 1))
        self.assertIn("cap", str(caught.exception))

    def test_a_non_string_text_is_refused(self):
        with self.assertRaises(LinearPatchError):
            lp.apply_ops(BODY, [{"op": "append", "text": 7}])

    def test_a_non_array_ops_file_is_refused(self):
        with self.assertRaises(LinearPatchError):
            lp.apply_ops(BODY, {"op": "append", "text": "x"})


class ResolveStateTests(unittest.TestCase):
    NODES = [{"id": "u1", "name": "Todo"}, {"id": "u2", "name": "In Review"}]

    def test_a_name_resolves_to_a_uuid_case_insensitively(self):
        self.assertEqual(lp.resolve_state(self.NODES, "in review"), "u2")

    def test_an_unknown_name_names_the_available_states(self):
        with self.assertRaises(LinearPatchError) as caught:
            lp.resolve_state(self.NODES, "Shipped")
        self.assertIn("In Review", str(caught.exception))


class ZeroEchoTests(unittest.TestCase):
    """Nothing printed may contain the stored body."""

    SECRET = "UNIQUE-BODY-SENTINEL"

    def _issue(self, description: str) -> dict:
        return {
            "id": "uuid-1",
            "identifier": "ENG-942",
            "url": "https://linear.app/x/ENG-942",
            "description": description,
            "team": {"id": "team-1"},
            "state": {"name": "Todo"},
        }

    def _run(self, argv: list[str], posts: list) -> str:
        out, err = io.StringIO(), io.StringIO()
        with (
            mock.patch.dict(os.environ, {"LINEAR_API_KEY": "k"}),
            mock.patch.object(lp, "_post", side_effect=posts),
        ):
            with redirect_stdout(out), redirect_stderr(err):
                lp.run(argv)
        return out.getvalue() + err.getvalue()

    def test_patch_prints_a_size_delta_not_the_body(self):
        with tempfile.TemporaryDirectory() as d:
            ops = os.path.join(d, "ops.json")
            with open(ops, "w", encoding="utf-8") as handle:
                json.dump([{"op": "append", "text": "\nnew"}], handle)
            printed = self._run(
                ["patch", "ENG-942", "--ops", ops],
                [
                    {"issue": self._issue(self.SECRET)},
                    {
                        "issueUpdate": {
                            "success": True,
                            "issue": {"identifier": "ENG-942", "url": "u"},
                        }
                    },
                ],
            )
        self.assertNotIn(self.SECRET, printed)
        self.assertIn("PATCHED", printed)
        self.assertIn("chars", printed)

    def test_read_writes_the_body_to_a_file_and_prints_only_its_size(self):
        with tempfile.TemporaryDirectory() as d:
            out_path = os.path.join(d, "body.md")
            printed = self._run(
                ["read", "ENG-942", "--out", out_path],
                [{"issue": self._issue(self.SECRET)}],
            )
            with open(out_path, encoding="utf-8") as handle:
                self.assertEqual(handle.read(), self.SECRET)
        self.assertNotIn(self.SECRET, printed)
        self.assertIn(str(len(self.SECRET)), printed)

    def test_a_state_transition_reads_no_body_into_its_output(self):
        printed = self._run(
            ["state", "ENG-942", "--state", "In Review"],
            [
                {"issue": self._issue(self.SECRET)},
                {"team": {"states": {"nodes": [{"id": "u2", "name": "In Review"}]}}},
                {
                    "issueUpdate": {
                        "success": True,
                        "issue": {
                            "identifier": "ENG-942",
                            "url": "u",
                            "state": {"name": "In Review"},
                        },
                    }
                },
            ],
        )
        self.assertNotIn(self.SECRET, printed)
        self.assertIn("SET", printed)

    def test_a_no_op_state_write_is_skipped_entirely(self):
        printed = self._run(
            ["state", "ENG-942", "--state", "todo"],
            [{"issue": self._issue(self.SECRET)}],
        )
        self.assertIn("ALREADY", printed)

    def test_comment_prints_a_url_and_a_size_not_the_text(self):
        with tempfile.TemporaryDirectory() as d:
            body_path = os.path.join(d, "note.md")
            with open(body_path, "w", encoding="utf-8") as handle:
                handle.write("NARRATIVE-SENTINEL")
            printed = self._run(
                ["comment", "ENG-942", "--body", body_path],
                [
                    {"issue": self._issue(self.SECRET)},
                    {"commentCreate": {"success": True, "comment": {"url": "c"}}},
                ],
            )
        self.assertNotIn(self.SECRET, printed)
        self.assertNotIn("NARRATIVE-SENTINEL", printed)
        self.assertIn("COMMENTED", printed)

    def test_a_patch_that_changes_nothing_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            ops = os.path.join(d, "ops.json")
            with open(ops, "w", encoding="utf-8") as handle:
                json.dump([{"op": "append", "text": ""}], handle)
            printed = self._run(
                ["patch", "ENG-942", "--ops", ops],
                [{"issue": self._issue(self.SECRET)}],
            )
        self.assertIn("changes nothing", printed)

    def test_a_dry_run_writes_nothing_and_still_reports_the_delta(self):
        with tempfile.TemporaryDirectory() as d:
            ops = os.path.join(d, "ops.json")
            with open(ops, "w", encoding="utf-8") as handle:
                json.dump([{"op": "append", "text": "\nnew"}], handle)
            # Exactly one _post: the body read. A second would be a write.
            printed = self._run(
                ["patch", "ENG-942", "--ops", ops, "--dry-run"],
                [{"issue": self._issue(self.SECRET)}],
            )
        self.assertIn("WOULD PATCH", printed)
        self.assertNotIn(self.SECRET, printed)

    def test_an_unresolved_issue_is_an_error_not_a_silent_no_op(self):
        printed = self._run(["state", "ENG-942", "--state", "Done"], [{"issue": None}])
        self.assertIn("did not resolve", printed)


if __name__ == "__main__":
    unittest.main()
