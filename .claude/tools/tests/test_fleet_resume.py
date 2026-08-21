#!/usr/bin/env python3
"""Unit tests for ``fleet_resume.py`` (stdlib ``unittest``; no pytest).

Nothing here talks to Linear or to iTerm: the two seams (``_post`` and
``_osascript``) are patched, and the assertions are on the plan the tool would
carry out and on the AppleScript it would emit.

**What these tests do not cover, deliberately.** The ``--apply`` path's *effect*
— tabs actually appearing — cannot be asserted without opening tabs in the
operator's live window and resuming real work sessions. The emitted script is
verified instead by compiling it against the iTerm scripting dictionary with
``osacompile``, which resolves every term without executing anything; see
:class:`EmittedScriptIsValid`.
"""

from __future__ import annotations

import io
import json
import os
import shutil
import subprocess
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

import fleet_resume as fr


def _issue(identifier="ENG-889", state="In Progress", title="A task"):
    return {
        "identifier": identifier,
        "title": title,
        "state": {"name": state, "type": "started"},
    }


def _page(nodes, has_next=False, cursor=None):
    return {
        "issues": {
            "pageInfo": {"hasNextPage": has_next, "endCursor": cursor},
            "nodes": nodes,
        }
    }


class TagOf(unittest.TestCase):
    def test_it_strips_the_prefix(self):
        self.assertEqual(fr.tag_of("ENG-889"), "889")
        self.assertEqual(fr.tag_of("eng-12"), "12")
        self.assertEqual(fr.tag_of("  ENG-7  "), "7")

    def test_a_non_eng_identifier_yields_none(self):
        # Skipped rather than turned into a bad `raps` argument.
        self.assertIsNone(fr.tag_of("OPS-4"))
        self.assertIsNone(fr.tag_of("ENG-"))
        self.assertIsNone(fr.tag_of(""))


class LiveTags(unittest.TestCase):
    def test_it_reads_the_tag_out_of_a_tab_name(self):
        # The name carries a status glyph, so this is a search not a match.
        names = "◐ eng-914\n✳ eng-923\n"
        with mock.patch.object(fr, "_osascript", return_value=names):
            self.assertEqual(fr.live_tags(), {"914", "923"})

    def test_a_tab_with_no_tag_contributes_nothing(self):
        # A plain shell, or a planning session — neither is resumed by this.
        names = "Default\n✳ plan-21\nbash\n"
        with mock.patch.object(fr, "_osascript", return_value=names):
            self.assertEqual(fr.live_tags(), set())

    def test_no_open_windows_is_an_empty_set_not_an_error(self):
        with mock.patch.object(fr, "_osascript", return_value=""):
            self.assertEqual(fr.live_tags(), set())


class Plan(unittest.TestCase):
    def _plan(self, issues, live):
        with (
            mock.patch.object(fr, "_post", return_value=_page(issues)),
            mock.patch.object(fr, "live_tags", return_value=live),
        ):
            return fr.plan("key", "proj")

    def test_an_issue_with_no_live_tab_is_resumed(self):
        result = self._plan([_issue("ENG-889")], set())
        self.assertEqual([e["tag"] for e in result["resume"]], ["889"])
        self.assertEqual(result["skipped_already_live"], [])

    def test_an_issue_with_a_live_tab_is_skipped_not_double_resumed(self):
        result = self._plan([_issue("ENG-889")], {"889"})
        self.assertEqual(result["resume"], [])
        self.assertEqual(
            [e["identifier"] for e in result["skipped_already_live"]], ["ENG-889"]
        )

    def test_a_mixed_fleet_splits_correctly(self):
        result = self._plan([_issue("ENG-1"), _issue("ENG-2"), _issue("ENG-3")], {"2"})
        self.assertEqual([e["tag"] for e in result["resume"]], ["1", "3"])
        self.assertEqual(len(result["skipped_already_live"]), 1)
        self.assertEqual(result["in_flight"], 3)

    def test_an_unrecognized_identifier_is_reported_not_resumed(self):
        result = self._plan([_issue("OPS-4")], set())
        self.assertEqual(result["resume"], [])
        self.assertEqual(len(result["unrecognized_identifier"]), 1)

    def test_it_filters_on_the_started_state_type(self):
        # The type, not the state names: In Progress and In Review both mean a
        # session owns the issue, and a workflow rename must not drop one.
        seen = []

        def fake(api_key, query, variables):
            seen.append(variables["filter"])
            return _page([])

        with (
            mock.patch.object(fr, "_post", side_effect=fake),
            mock.patch.object(fr, "live_tags", return_value=set()),
        ):
            fr.plan("key", "proj")
        self.assertEqual(seen[0]["state"], {"type": {"eq": "started"}})
        self.assertEqual(seen[0]["project"], {"id": {"eq": "proj"}})

    def test_it_follows_the_cursor(self):
        pages = [_page([_issue("ENG-1")], True, "c1"), _page([_issue("ENG-2")])]
        calls = []

        def fake(api_key, query, variables):
            calls.append(variables.get("after"))
            return pages[len(calls) - 1]

        with (
            mock.patch.object(fr, "_post", side_effect=fake),
            mock.patch.object(fr, "live_tags", return_value=set()),
        ):
            result = fr.plan("key", "proj")
        self.assertEqual([e["tag"] for e in result["resume"]], ["1", "2"])
        self.assertEqual(calls, [None, "c1"])


class OpenScript(unittest.TestCase):
    def test_it_writes_the_resume_verb_and_presses_enter(self):
        script = fr.open_script(["889"])
        self.assertIn('write s text "raps 889" newline yes', script)

    def test_newline_is_explicit_not_defaulted(self):
        # The operator's ask is that Enter is actually pressed; relying on
        # `write text`'s default newline would make that implicit.
        self.assertIn("newline yes", fr.open_script(["1"]))

    def test_it_emits_one_script_for_the_whole_fleet(self):
        script = fr.open_script(["1", "2", "3"])
        self.assertEqual(script.count('tell application "iTerm2"'), 1)
        self.assertEqual(script.count("create tab with default profile"), 3)

    def test_it_reports_each_tab_s_tty(self):
        # The tty is how the attend mark reaches a tab this process is not in.
        script = fr.open_script(["889"])
        self.assertIn("tty of s", script)

    def test_a_value_is_quoted_for_applescript(self):
        self.assertEqual(fr._applescript_literal('a"b'), '"a\\"b"')
        self.assertEqual(fr._applescript_literal("a\\b"), '"a\\\\b"')

    def test_no_tags_still_produces_a_valid_shell_of_a_script(self):
        script = fr.open_script([])
        self.assertIn('tell application "iTerm2"', script)
        self.assertNotIn("create tab", script)


class ParseOpenResult(unittest.TestCase):
    def test_it_pairs_tags_with_ttys(self):
        out = "889 /dev/ttys004\n852 /dev/ttys005\n"
        self.assertEqual(
            fr.parse_open_result(out),
            [("889", "/dev/ttys004"), ("852", "/dev/ttys005")],
        )

    def test_a_malformed_line_is_dropped_not_guessed_at(self):
        out = "889 /dev/ttys004\nnonsense\n852 not-a-tty\n\n"
        self.assertEqual(fr.parse_open_result(out), [("889", "/dev/ttys004")])


class Summary(unittest.TestCase):
    def test_it_states_the_three_counts(self):
        line = fr.summarize(
            {
                "in_flight": 4,
                "resume": [{}, {}, {}],
                "skipped_already_live": [{}],
                "unrecognized_identifier": [],
            }
        )
        self.assertIn("4 in flight", line)
        self.assertIn("3 to resume", line)
        self.assertIn("1 already live", line)

    def test_it_reports_tabs_that_could_not_be_marked(self):
        line = fr.summarize(
            {
                "in_flight": 1,
                "resume": [{}],
                "skipped_already_live": [],
                "unrecognized_identifier": [],
                "opened": 1,
                "unmarked": ["889"],
            }
        )
        self.assertIn("1 opened", line)
        self.assertIn("could not be marked", line)


class Cli(unittest.TestCase):
    def setUp(self):
        self._env = mock.patch.dict(
            os.environ, {"LINEAR_API_KEY": "k", "LINEAR_PROJECT_ID": "p"}
        )
        self._env.start()
        self.addCleanup(self._env.stop)

    def _run(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = fr.run(["fleet_resume.py", *argv])
        return code, out.getvalue(), err.getvalue()

    def test_a_bare_run_opens_nothing(self):
        with (
            mock.patch.object(fr, "_post", return_value=_page([_issue("ENG-889")])),
            mock.patch.object(fr, "live_tags", return_value=set()),
            mock.patch.object(fr, "_osascript") as osa,
        ):
            code, out, err = self._run()
        self.assertEqual(code, 0)
        osa.assert_not_called()
        self.assertIn("read-only", err)
        self.assertEqual(json.loads(out)["resume"][0]["tag"], "889")

    def test_apply_opens_and_marks(self):
        with (
            mock.patch.object(fr, "_post", return_value=_page([_issue("ENG-889")])),
            mock.patch.object(fr, "live_tags", return_value=set()),
            mock.patch.object(fr, "_osascript", return_value="889 /dev/ttys009\n"),
            mock.patch.object(fr, "mark_attention", return_value=True) as marker,
        ):
            code, out, _ = self._run("--apply")
        self.assertEqual(code, 0)
        marker.assert_called_once_with("/dev/ttys009")
        parsed = json.loads(out)
        self.assertEqual(parsed["opened"], 1)
        self.assertEqual(parsed["unmarked"], [])

    def test_apply_with_nothing_to_resume_runs_no_applescript(self):
        with (
            mock.patch.object(fr, "_post", return_value=_page([_issue("ENG-889")])),
            mock.patch.object(fr, "live_tags", return_value={"889"}),
            mock.patch.object(fr, "_osascript") as osa,
        ):
            code, out, _ = self._run("--apply")
        self.assertEqual(code, 0)
        osa.assert_not_called()
        self.assertEqual(json.loads(out)["opened"], 0)

    def test_a_failed_mark_is_reported_not_fatal(self):
        with (
            mock.patch.object(fr, "_post", return_value=_page([_issue("ENG-889")])),
            mock.patch.object(fr, "live_tags", return_value=set()),
            mock.patch.object(fr, "_osascript", return_value="889 /dev/ttys009\n"),
            mock.patch.object(fr, "mark_attention", return_value=False),
        ):
            code, out, err = self._run("--apply")
        # The tab is open and resumed either way; only the tint is missing.
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(out)["unmarked"], ["889"])
        self.assertIn("could not be marked", err)

    def test_a_missing_env_var_raises_a_user_facing_error(self):
        with mock.patch.dict(os.environ, {"LINEAR_API_KEY": ""}):
            with self.assertRaises(fr.FleetResumeError) as caught:
                fr.run(["fleet_resume.py"])
        self.assertIn("LINEAR_API_KEY", str(caught.exception))

    def test_main_maps_that_error_to_exit_one_rather_than_a_traceback(self):
        err = io.StringIO()
        with mock.patch.dict(os.environ, {"LINEAR_API_KEY": ""}):
            with mock.patch.object(fr.sys, "argv", ["fleet_resume.py"]):
                with redirect_stderr(err):
                    code = fr.main()
        self.assertEqual(code, 1)
        self.assertIn("LINEAR_API_KEY", err.getvalue())


@unittest.skipUnless(shutil.which("osacompile"), "macOS-only: needs osacompile")
class EmittedScriptIsValid(unittest.TestCase):
    """The apply path's *effect* cannot be asserted without opening tabs in a
    live window, so the emitted script is verified by compiling it instead —
    `osacompile` resolves every term against the iTerm dictionary and runs
    nothing."""

    def test_the_emitted_script_compiles(self):
        completed = subprocess.run(
            ["osacompile", "-o", "/dev/null", "-"],
            input=fr.open_script(["923", "852"]),
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(
            completed.returncode, 0, f"osacompile said: {completed.stderr}"
        )

    def test_the_session_listing_script_compiles_too(self):
        completed = subprocess.run(
            ["osacompile", "-o", "/dev/null", "-"],
            input=fr._LIST_SESSIONS,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(
            completed.returncode, 0, f"osacompile said: {completed.stderr}"
        )


if __name__ == "__main__":
    unittest.main()
