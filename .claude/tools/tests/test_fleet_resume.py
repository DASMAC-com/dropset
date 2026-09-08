#!/usr/bin/env python3
"""Unit tests for ``fleet_resume.py`` (stdlib ``unittest``; no pytest).

Nothing here talks to Linear or to iTerm: the two seams (``_post`` and the
``iterm_api`` entry points) are patched, and the assertions are on the plan the
tool would carry out and the verbs it would type.

**What these tests do not cover, deliberately.** The ``--apply`` path's *effect*
— tabs actually appearing — cannot be asserted without opening tabs in the
operator's live window and resuming real work sessions.

This suite used to also compile the emitted AppleScript with ``osacompile`` as a
stand-in for that. The tool no longer emits AppleScript — iTerm is driven
through its Python API via the shared ``iterm_api`` module — so what those cases
checked (that every term resolves against the iTerm dictionary) has no analogue
here and is not replaced by a mock pretending to be one. The API surface this
now depends on is exercised for real by the live dispatch path, not by a fake.
"""

from __future__ import annotations

import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
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
    def test_it_reads_the_tag_out_of_a_session_name(self):
        # The name carries a status glyph, so this is a search not a match.
        # These are the real shapes, copied from a live listing.
        names = ["◐ eng-914", "◑ eng-923"]
        with mock.patch.object(fr.iterm_api, "session_names", return_value=names):
            self.assertEqual(fr.live_tags(), {"914", "923"})

    def test_a_session_with_no_tag_contributes_nothing(self):
        # A plain shell, or a planning session — neither is resumed by this.
        names = ["Default", "◐ plan-21", "bash"]
        with mock.patch.object(fr.iterm_api, "session_names", return_value=names):
            self.assertEqual(fr.live_tags(), set())

    def test_no_open_windows_is_an_empty_set_not_an_error(self):
        with mock.patch.object(fr.iterm_api, "session_names", return_value=[]):
            self.assertEqual(fr.live_tags(), set())

    def test_an_unreachable_iterm_reports_and_yields_an_empty_set(self):
        # The failure DIRECTION is the assertion. Empty means "nothing looks
        # live", so --apply would open a duplicate tab per issue: visible, and
        # cheap to close. The opposite default — unreachable reads as
        # everything-live — would resume nothing and print a clean summary,
        # which is the failure nobody notices.
        err = io.StringIO()
        with (
            mock.patch.object(
                fr.iterm_api,
                "session_names",
                side_effect=fr.iterm_api.ItermUnavailable("API off"),
            ),
            redirect_stderr(err),
        ):
            self.assertEqual(fr.live_tags(), set())
        self.assertIn("API off", err.getvalue())


class ResumeCommand(unittest.TestCase):
    def test_it_types_the_substrate_aware_resume_verb(self):
        # `task resume` reads the session's own substrate marker, which is what
        # lets a mixed Bedrock/seat fleet come back correctly with no substrate
        # knowledge in this tool.
        self.assertEqual(fr.resume_command("889"), "task resume 889")

    def test_open_tabs_pairs_each_tag_with_its_tty(self):
        with mock.patch.object(
            fr.iterm_api, "open_tabs", return_value=["/dev/ttys4", "/dev/ttys5"]
        ) as opened:
            pairs = fr.open_tabs(["889", "852"])
        self.assertEqual(pairs, [("889", "/dev/ttys4"), ("852", "/dev/ttys5")])
        opened.assert_called_once_with(["task resume 889", "task resume 852"])

    def test_a_tag_with_no_tty_is_omitted_from_the_pairs(self):
        # Omitted rather than carried with a placeholder, so the pair list keeps
        # meaning "these can be marked"; `run` reconstructs the shortfall by
        # difference against what it requested.
        with mock.patch.object(
            fr.iterm_api, "open_tabs", return_value=["/dev/ttys4", None]
        ):
            self.assertEqual(fr.open_tabs(["889", "852"]), [("889", "/dev/ttys4")])


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

    def test_a_next_page_with_no_cursor_raises_instead_of_looping(self):
        # Unreachable against a Relay-conforming server, but the failure mode
        # is an infinite loop re-issuing the identical query.
        with (
            mock.patch.object(fr, "_post", return_value=_page([], True, None)),
            mock.patch.object(fr, "live_tags", return_value=set()),
        ):
            with self.assertRaises(fr.FleetResumeError) as caught:
                fr.plan("key", "proj")
        self.assertIn("no cursor", str(caught.exception))

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

    def test_the_unmarked_slot_carries_a_COUNT_and_the_tags(self):
        # It used to interpolate the list itself into a slot reading "N could
        # not be marked", printing a raw Python list repr where a count belongs.
        line = fr.summarize(
            {
                "in_flight": 2,
                "resume": [{}, {}],
                "skipped_already_live": [],
                "unrecognized_identifier": [],
                "opened": 2,
                "unmarked": ["889", "1042"],
            }
        )
        self.assertIn("2 could not be marked", line)
        self.assertIn("889, 1042", line)
        self.assertNotIn("['889'", line)

    def test_tabs_opened_without_a_tty_are_reported(self):
        # The silent path that actually bit: no tty came back, so nothing was
        # marked AND `unmarked` was empty, giving a clean-looking summary over
        # a total mark failure.
        line = fr.summarize(
            {
                "in_flight": 2,
                "resume": [{}, {}],
                "skipped_already_live": [],
                "unrecognized_identifier": [],
                "opened": 0,
                "unmarked": [],
                "no_tty": ["889", "1042"],
                "requested": 3,
            }
        )
        self.assertIn("2 of 3 requested produced no tty", line)
        self.assertIn("889, 1042", line)

    def test_the_denominator_is_omitted_when_requested_is_unknown(self):
        # Defaulting it to the numerator would assert that EVERY requested tab
        # failed — an unverified claim of the same species as the "opened"
        # wording this replaced.
        line = fr.summarize(
            {
                "in_flight": 1,
                "resume": [{}],
                "skipped_already_live": [],
                "unrecognized_identifier": [],
                "opened": 0,
                "unmarked": [],
                "no_tty": ["889"],
            }
        )
        self.assertIn("1 produced no tty", line)
        self.assertNotIn("requested", line)

    def test_a_fully_marked_run_stays_quiet(self):
        line = fr.summarize(
            {
                "in_flight": 1,
                "resume": [{}],
                "skipped_already_live": [],
                "unrecognized_identifier": [],
                "opened": 1,
                "unmarked": [],
                "no_tty": [],
            }
        )
        # A positive assertion too, so the test cannot pass on an empty
        # summary — both checks below are negative, and `return ""` would
        # satisfy them on its own.
        self.assertIn("1 opened", line)
        self.assertNotIn("could not be marked", line)
        self.assertNotIn("produced no tty", line)

    def test_unrecognized_identifiers_are_surfaced_when_present(self):
        """The one branch of `summarize` nothing else covers.

        An unrecognized identifier is an issue whose key `tag_of` could not
        parse, which means it is silently absent from both `resume` and
        `skipped_already_live` — the summary line is the only place it is ever
        mentioned. A dropped `if` here would read as a clean fleet.
        """
        line = fr.summarize(
            {
                "in_flight": 3,
                "resume": [{}],
                "skipped_already_live": [{}],
                "unrecognized_identifier": ["OPS-7"],
            }
        )
        self.assertIn("1 unrecognized", line)

    def test_no_unrecognized_clause_when_the_list_is_empty(self):
        """The counterpart: a clean fleet must not carry a `0 unrecognized`."""
        line = fr.summarize(
            {
                "in_flight": 1,
                "resume": [{}],
                "skipped_already_live": [],
                "unrecognized_identifier": [],
            }
        )
        self.assertNotIn("unrecognized", line)


class MarkAttention(unittest.TestCase):
    """The argv `mark_attention` passes is the ONLY consumer of the two flags
    this PR adds to `iterm-attend.sh`, and shell is untested by design here —
    so without these cases nothing anywhere asserts the two sides agree.

    Note this is NOT the declared `--apply`-effect gap: asserting the argv
    opens no tabs and resumes no sessions.
    """

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = self._tmp.name

    def _stub(self, body: str, mode: int = 0o755) -> str:
        path = os.path.join(self.root, "iterm-attend.sh")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(body)
        os.chmod(path, mode)
        return path

    def test_it_passes_the_tty_and_the_mark_flag(self):
        record = os.path.join(self.root, "argv.txt")
        stub = self._stub('#!/bin/sh\nprintf "%s\\n" "$@" > ' + record + "\nexit 0\n")
        with mock.patch.object(fr, "_ATTEND", Path(stub)):
            self.assertTrue(fr.mark_attention("/dev/ttys009"))
        with open(record, encoding="utf-8") as fh:
            argv = fh.read().split()
        # Exactly the contract iterm-attend.sh's own arg loop parses.
        self.assertEqual(argv, ["--tty", "/dev/ttys009", "--mark"])

    def test_a_non_zero_exit_reports_false(self):
        stub = self._stub("#!/bin/sh\nexit 3\n")
        with mock.patch.object(fr, "_ATTEND", Path(stub)):
            self.assertFalse(fr.mark_attention("/dev/ttys009"))

    def test_a_missing_script_reports_false(self):
        with mock.patch.object(fr, "_ATTEND", Path(self.root) / "gone.sh"):
            self.assertFalse(fr.mark_attention("/dev/ttys009"))

    def test_a_non_executable_script_reports_false_rather_than_raising(self):
        # `.exists()` is not `.access(X_OK)`. Uncaught, this raised a traceback
        # after every tab was already open and resumed.
        stub = self._stub("#!/bin/sh\nexit 0\n", mode=0o644)
        with mock.patch.object(fr, "_ATTEND", Path(stub)):
            self.assertFalse(fr.mark_attention("/dev/ttys009"))


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
            mock.patch.object(fr.iterm_api, "open_tabs") as opened,
        ):
            code, out, err = self._run()
        self.assertEqual(code, 0)
        opened.assert_not_called()
        self.assertIn("read-only", err)
        self.assertEqual(json.loads(out)["resume"][0]["tag"], "889")

    def test_apply_opens_and_marks(self):
        with (
            mock.patch.object(fr, "_post", return_value=_page([_issue("ENG-889")])),
            mock.patch.object(fr, "live_tags", return_value=set()),
            mock.patch.object(fr.iterm_api, "open_tabs", return_value=["/dev/ttys009"]),
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
            mock.patch.object(fr.iterm_api, "open_tabs") as opened,
        ):
            code, out, _ = self._run("--apply")
        self.assertEqual(code, 0)
        opened.assert_not_called()
        self.assertEqual(json.loads(out)["opened"], 0)

    def test_a_failed_mark_is_reported_not_fatal(self):
        with (
            mock.patch.object(fr, "_post", return_value=_page([_issue("ENG-889")])),
            mock.patch.object(fr, "live_tags", return_value=set()),
            mock.patch.object(fr.iterm_api, "open_tabs", return_value=["/dev/ttys009"]),
            mock.patch.object(fr, "mark_attention", return_value=False),
        ):
            code, out, err = self._run("--apply")
        # The tab is open and resumed either way; only the tint is missing.
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(out)["unmarked"], ["889"])
        self.assertIn("could not be marked", err)

    def test_tabs_that_report_no_tty_are_tracked_against_what_was_requested(self):
        # The measured silent failure: the AppleScript ran and tabs opened, but
        # nothing parsed as a tty. `unmarked` stays empty because it is derived
        # from what DID parse, so without `no_tty` the run reports total
        # success over a total mark failure.
        with (
            mock.patch.object(fr, "_post", return_value=_page([_issue("ENG-889")])),
            mock.patch.object(fr, "live_tags", return_value=set()),
            mock.patch.object(fr.iterm_api, "open_tabs", return_value=[None]),
            mock.patch.object(fr, "mark_attention", return_value=True) as marker,
        ):
            code, out, err = self._run("--apply")
        self.assertEqual(code, 0)
        marker.assert_not_called()
        parsed = json.loads(out)
        self.assertEqual(parsed["opened"], 0)
        self.assertEqual(parsed["unmarked"], [])
        self.assertEqual(parsed["no_tty"], ["889"])
        self.assertEqual(parsed["requested"], 1)
        self.assertIn("1 of 1 requested produced no tty", err)

    def test_a_partial_tty_shortfall_is_reported(self):
        with (
            mock.patch.object(
                fr,
                "_post",
                return_value=_page([_issue("ENG-889"), _issue("ENG-1042")]),
            ),
            mock.patch.object(fr, "live_tags", return_value=set()),
            mock.patch.object(fr.iterm_api, "open_tabs", return_value=["/dev/ttys009"]),
            mock.patch.object(fr, "mark_attention", return_value=True),
        ):
            code, out, err = self._run("--apply")
        self.assertEqual(code, 0)
        parsed = json.loads(out)
        self.assertEqual(parsed["opened"], 1)
        self.assertEqual(parsed["no_tty"], ["1042"])
        self.assertIn("1042", err)

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


if __name__ == "__main__":
    unittest.main()
