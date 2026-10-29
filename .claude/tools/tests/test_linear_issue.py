#!/usr/bin/env python3
"""Unit tests for ``linear_issue.py`` (stdlib ``unittest``; no pytest).

The GraphQL transport is replaced with a recorder, so these assert the thing
that cannot be checked by eye and is the entire reason the tool exists: that
the stored issue body never reaches stdout, and that what does reach the wire
is the grown body rather than a replacement.
"""

from __future__ import annotations

import io
import os
import tempfile
import unittest
from contextlib import redirect_stdout

import linear_issue as li


class _Recorder:
    """Stands in for the GraphQL transport, recording what was sent."""

    def __init__(self, responses):
        self.responses = list(responses)
        self.calls = []

    def __call__(self, api_key, query, variables):
        self.calls.append((query, variables))
        if not self.responses:
            raise AssertionError("unexpected extra GraphQL call")
        return self.responses.pop(0)


class _Stubbed(unittest.TestCase):
    def setUp(self):
        real = li._post
        self.addCleanup(lambda: setattr(li, "_post", real))

    def install(self, responses):
        recorder = _Recorder(responses)
        li._post = recorder
        return recorder


class AppendBody(_Stubbed):
    @staticmethod
    def body(description):
        return {
            "issue": {
                "id": "u1",
                "identifier": "ENG-1",
                "url": "https://example.invalid/ENG-1",
                "description": description,
            }
        }

    @staticmethod
    def ok():
        return {"issueUpdate": {"success": True, "issue": {"identifier": "ENG-1"}}}

    def test_the_stored_body_never_reaches_the_returned_line(self):
        # The whole point of the tool. The body is read into this process,
        # grown, and sent back; what comes out is accounting.
        secret = "STORED BODY THAT MUST NOT BE ECHOED"
        self.install([self.body(secret), self.ok()])
        line = li.append_body("k", "ENG-1", "fresh evidence", dry_run=False)
        self.assertNotIn(secret, line)
        self.assertIn("APPENDED to ENG-1", line)

    def test_the_addition_is_separated_by_exactly_one_blank_line(self):
        recorder = self.install([self.body("first\n"), self.ok()])
        li.append_body("k", "ENG-1", "second", dry_run=False)
        self.assertEqual(recorder.calls[-1][1]["description"], "first\n\nsecond\n")

    def test_the_stored_body_is_preserved_not_replaced(self):
        # An append that dropped the prior body would still print a plausible
        # success line, so the retained prefix is asserted directly.
        recorder = self.install([self.body("keep me"), self.ok()])
        li.append_body("k", "ENG-1", "added", dry_run=False)
        self.assertTrue(recorder.calls[-1][1]["description"].startswith("keep me"))

    def test_an_empty_body_does_not_gain_leading_blank_lines(self):
        recorder = self.install([self.body(""), self.ok()])
        li.append_body("k", "ENG-1", "only", dry_run=False)
        self.assertEqual(recorder.calls[-1][1]["description"], "only\n")

    def test_a_null_description_is_treated_as_empty(self):
        # Linear returns null, not "", for an issue filed with no body.
        recorder = self.install([self.body(None), self.ok()])
        li.append_body("k", "ENG-1", "only", dry_run=False)
        self.assertEqual(recorder.calls[-1][1]["description"], "only\n")

    def test_empty_text_is_refused_rather_than_written(self):
        recorder = self.install([])
        with self.assertRaises(li.LinearIssueError):
            li.append_body("k", "ENG-1", "   \n  ", dry_run=False)
        self.assertEqual(recorder.calls, [])

    def test_dry_run_does_not_even_read_the_body(self):
        recorder = self.install([])
        line = li.append_body("k", "ENG-1", "text", dry_run=True)
        self.assertTrue(line.startswith("WOULD APPEND"))
        self.assertEqual(recorder.calls, [])

    def test_a_reported_failure_raises_rather_than_claiming_success(self):
        self.install([self.body("x"), {"issueUpdate": {"success": False}}])
        with self.assertRaises(li.LinearIssueError):
            li.append_body("k", "ENG-1", "text", dry_run=False)

    def test_a_missing_issue_is_an_error_not_a_silent_skip(self):
        self.install([{"issue": None}])
        with self.assertRaises(li.LinearIssueError):
            li.append_body("k", "ENG-404", "text", dry_run=False)


class Find(_Stubbed):
    @staticmethod
    def rows(nodes):
        return {"issues": {"nodes": nodes}}

    def test_only_identifier_and_title_come_back(self):
        self.install(
            [self.rows([{"identifier": "ENG-7", "title": "Reference price guard"}])]
        )
        got = li.find("k", query="reference", project_id=None, state=None, limit=5)
        self.assertEqual(got, ["ENG-7  Reference price guard"])

    def test_the_title_predicate_is_sent(self):
        # The one clause that is never optional, and the entire reason the
        # subcommand exists. Without this assertion the base filter could be
        # replaced with `{}` — a search returning every issue in the workspace,
        # truncated to `first` — and every other Find test would stay green.
        recorder = self.install([self.rows([])])
        li.find("k", query="reference price", project_id=None, state=None, limit=5)
        sent = recorder.calls[-1][1]["filter"]
        self.assertEqual(sent["title"], {"containsIgnoreCase": "reference price"})

    def test_the_filters_are_sent_when_given(self):
        recorder = self.install([self.rows([])])
        li.find("k", query="q", project_id="p1", state="Backlog", limit=5)
        sent = recorder.calls[-1][1]["filter"]
        self.assertEqual(sent["project"], {"id": {"eq": "p1"}})
        self.assertEqual(sent["state"], {"name": {"eq": "Backlog"}})

    def test_the_limit_is_floored_as_well_as_capped(self):
        # `first: 0` or a negative returns a raw GraphQL error rather than the
        # one clean stderr line this module promises.
        recorder = self.install([self.rows([])])
        li.find("k", query="q", project_id=None, state=None, limit=0)
        self.assertEqual(recorder.calls[-1][1]["first"], 1)

    def test_the_cli_accepts_dry_run_after_the_subcommand(self):
        # The form the module's own Usage block teaches. Registered only on the
        # parent parser it exited 2 with "unrecognized arguments" — and this
        # tool's only write guard was unreachable in its documented spelling.
        args = li._parse_args(
            ["linear_issue.py", "append", "--id", "ENG-1", "--text", "x", "--dry-run"]
        )
        self.assertTrue(args.dry_run)

    def test_the_cli_accepts_dry_run_before_the_subcommand(self):
        # And the subparser's default must not overwrite it back to False.
        args = li._parse_args(
            ["linear_issue.py", "--dry-run", "append", "--id", "ENG-1", "--text", "x"]
        )
        self.assertTrue(args.dry_run)

    def test_absent_filters_are_omitted_rather_than_sent_as_null(self):
        recorder = self.install([self.rows([])])
        li.find("k", query="q", project_id=None, state=None, limit=5)
        sent = recorder.calls[-1][1]["filter"]
        self.assertNotIn("project", sent)
        self.assertNotIn("state", sent)

    def test_the_limit_is_capped(self):
        recorder = self.install([self.rows([])])
        li.find("k", query="q", project_id=None, state=None, limit=10_000)
        self.assertEqual(recorder.calls[-1][1]["first"], li.MAX_FIND)

    def test_no_matches_is_an_empty_list_not_an_error(self):
        self.install([self.rows([])])
        self.assertEqual(
            li.find("k", query="q", project_id=None, state=None, limit=5), []
        )


class FindMany(_Stubbed):
    """Several queries in one call: one grouped block per query."""

    def setUp(self):
        super().setUp()
        saved = os.environ.get("LINEAR_API_KEY")
        os.environ["LINEAR_API_KEY"] = "k"
        self.addCleanup(
            lambda: (
                os.environ.pop("LINEAR_API_KEY")
                if saved is None
                else os.environ.__setitem__("LINEAR_API_KEY", saved)
            )
        )

    def run_cli(self, argv):
        out = io.StringIO()
        with redirect_stdout(out):
            self.assertEqual(li.run(["linear_issue.py", *argv]), 0)
        return out.getvalue().splitlines()

    def test_one_query_keeps_the_bare_row_shape(self):
        self.install([Find.rows([{"identifier": "ENG-7", "title": "Price guard"}])])
        self.assertEqual(
            self.run_cli(["find", "--query", "price"]), ["ENG-7  Price guard"]
        )

    def test_each_query_gets_its_own_group_and_count(self):
        recorder = self.install(
            [
                Find.rows([{"identifier": "ENG-7", "title": "Price guard"}]),
                Find.rows([]),
            ]
        )
        got = self.run_cli(["find", "--query", "price", "--query", "fold stamp"])
        self.assertEqual(
            got,
            [
                "## price — 1 match(es)",
                "ENG-7  Price guard",
                "## fold stamp — 0 match(es)",
            ],
        )
        sent = [
            call[1]["filter"]["title"]["containsIgnoreCase"] for call in recorder.calls
        ]
        self.assertEqual(sent, ["price", "fold stamp"])

    def test_a_queries_file_adds_to_the_flags_and_skips_blank_lines(self):
        recorder = self.install([Find.rows([]), Find.rows([]), Find.rows([])])
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
            fh.write("alpha\n\n  beta  \n")
        self.addCleanup(os.unlink, fh.name)
        self.run_cli(["find", "--query", "first", "--queries-file", fh.name])
        sent = [
            call[1]["filter"]["title"]["containsIgnoreCase"] for call in recorder.calls
        ]
        self.assertEqual(sent, ["first", "alpha", "beta"])

    def test_repeated_and_blank_queries_are_sent_once(self):
        recorder = self.install([Find.rows([]), Find.rows([])])
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
            fh.write("alpha\nbeta\n")
        self.addCleanup(os.unlink, fh.name)
        self.run_cli(
            ["find", "--query", " alpha ", "--query", "  ", "--queries-file", fh.name]
        )
        sent = [c[1]["filter"]["title"]["containsIgnoreCase"] for c in recorder.calls]
        self.assertEqual(sent, ["alpha", "beta"])

    def test_no_query_at_all_is_a_clean_error(self):
        self.install([])
        with self.assertRaises(li.LinearIssueError):
            li.run(["linear_issue.py", "find"])


class CreateIssue(_Stubbed):
    """The bulk filer. Its reason to exist is that `save_issue` echoes the
    whole stored body on a *create* too, which is worst for exactly the callers
    whose bodies are long by design — one audit session's 9 calls cost ≈22.8k
    and supplied six of its eight largest results."""

    CREATED = {
        "issueCreate": {
            "success": True,
            "issue": {"identifier": "ENG-500", "url": "https://x/ENG-500"},
        }
    }

    def test_it_files_in_one_call_and_echoes_no_body(self):
        rec = self.install([self.CREATED])
        line = li.create_issue(
            "k",
            team_id="team-1",
            project_id="proj-1",
            title="Audit: something",
            body="a long body with FINDINGS in it",
            dry_run=False,
        )
        self.assertEqual(len(rec.calls), 1, "one call — no filing then amending")
        sent = rec.calls[0][1]["input"]
        self.assertEqual(sent["title"], "Audit: something")
        self.assertIn("FINDINGS", sent["description"])
        # The whole point: the body went to the wire, not to stdout.
        self.assertNotIn("FINDINGS", line)
        self.assertIn("ENG-500", line)

    def test_a_plain_backlog_filing_reminds_to_message_the_curator(self):
        states = {"team": {"states": {"nodes": [{"id": "s-b", "name": "Backlog"}]}}}
        self.install([states, self.CREATED])
        line = li.create_issue(
            "k", team_id="t", project_id="p", title="t", body="b", state="backlog"
        )
        first, reminder = line.split("\n")
        self.assertIn("ENG-500", first)
        self.assertEqual(reminder, li.CURATOR_REMINDER)

    def test_a_parked_or_todo_filing_carries_no_reminder(self):
        states = {
            "team": {
                "states": {
                    "nodes": [
                        {"id": "s-b", "name": "Backlog"},
                        {"id": "s-t", "name": "Todo"},
                    ]
                }
            }
        }
        milestones = {
            "project": {
                "projectMilestones": {"nodes": [{"id": "m", "name": "Audit findings"}]}
            }
        }
        self.install([states, milestones, self.CREATED, states, self.CREATED])
        parked = li.create_issue(
            "k",
            team_id="t",
            project_id="p",
            title="t",
            body="b",
            state="Backlog",
            milestone="Audit findings",
        )
        todo = li.create_issue(
            "k", team_id="t", project_id="p", title="t", body="b", state="Todo"
        )
        self.assertNotIn("REMINDER", parked)
        self.assertNotIn("REMINDER", todo)

    def test_state_and_milestone_are_resolved_from_names(self):
        """`board_batch.py fields` deliberately refuses a milestone NAME, which
        is right for a field write; a filer that could not name its own
        milestone would push the lookup back into the transcript."""
        rec = self.install(
            [
                {"team": {"states": {"nodes": [{"id": "s-todo", "name": "Todo"}]}}},
                {
                    "project": {
                        "projectMilestones": {
                            "nodes": [{"id": "m-af", "name": "Audit findings"}]
                        }
                    }
                },
                self.CREATED,
            ]
        )
        li.create_issue(
            "k",
            team_id="team-1",
            project_id="proj-1",
            title="t",
            body="b",
            state="todo",  # case-insensitive
            milestone="Audit findings",
            priority=3,
        )
        sent = rec.calls[-1][1]["input"]
        self.assertEqual(sent["stateId"], "s-todo")
        self.assertEqual(sent["projectMilestoneId"], "m-af")
        self.assertEqual(sent["priority"], 3)

    def test_everything_rides_the_creating_call(self):
        """Filing then amending costs a second full echo and buys nothing."""
        rec = self.install(
            [
                {"team": {"states": {"nodes": [{"id": "s", "name": "Todo"}]}}},
                self.CREATED,
            ]
        )
        li.create_issue(
            "k",
            team_id="t",
            project_id="p",
            title="t",
            body="b",
            state="Todo",
            assignee_id="me",
        )
        create_calls = [c for c in rec.calls if "issueCreate" in c[0]]
        self.assertEqual(len(create_calls), 1)
        sent = create_calls[0][1]["input"]
        self.assertEqual(sent["assigneeId"], "me")
        self.assertEqual(sent["stateId"], "s")

    def test_an_unknown_state_name_lists_what_is_available(self):
        self.install([{"team": {"states": {"nodes": [{"id": "s", "name": "Todo"}]}}}])
        with self.assertRaises(li.LinearIssueError) as ctx:
            li.create_issue(
                "k",
                team_id="t",
                project_id="p",
                title="t",
                body="b",
                state="Nope",
            )
        self.assertIn("Todo", str(ctx.exception))

    def test_a_dry_run_writes_nothing(self):
        rec = self.install([])
        line = li.create_issue(
            "k",
            team_id="t",
            project_id="p",
            title="t",
            body="b",
            state="Todo",
            milestone="Audit findings",
            dry_run=True,
        )
        self.assertEqual(rec.calls, [])
        self.assertIn("WOULD FILE", line)
        self.assertIn("Audit findings", line)

    def test_a_blank_title_or_body_is_refused(self):
        for kwargs in ({"title": "  "}, {"body": ""}):
            with self.subTest(kwargs=kwargs), self.assertRaises(li.LinearIssueError):
                li.create_issue(
                    "k",
                    team_id="t",
                    project_id="p",
                    **{"title": "t", "body": "b", **kwargs},
                )

    def test_an_unsuccessful_create_is_an_error_not_a_silent_pass(self):
        self.install([{"issueCreate": {"success": False}}])
        with self.assertRaises(li.LinearIssueError):
            li.create_issue("k", team_id="t", project_id="p", title="t", body="b")

    def _dry(self, **token) -> str:
        rec = self.install([])
        line = li.create_issue(
            "k",
            team_id="t",
            project_id="p",
            title="Claude: Do x",
            body="b",
            dry_run=True,
            **token,
        )
        self.assertEqual(rec.calls, [])
        return line

    def test_verb_and_substrate_prefix_the_whole_token(self):
        line = self._dry(verb="chore", substrate="anthropic")
        self.assertIn("WOULD FILE [Chore/Anthropic] Claude: Do x |", line)

    def test_token_input_is_case_insensitive_and_written_canonically(self):
        line = self._dry(verb="EXPLORE", substrate="Bedrock")
        self.assertIn("WOULD FILE [Explore/Bedrock] Claude: Do x |", line)

    def test_task_on_anthropic_is_written_since_only_bedrock_is_elided(self):
        line = self._dry(verb="task", substrate="anthropic")
        self.assertIn("WOULD FILE [Task/Anthropic] Claude: Do x |", line)

    def test_operator_writes_a_substrate_less_token(self):
        line = self._dry(verb="Operator")
        self.assertIn("WOULD FILE [Operator] Claude: Do x |", line)

    def test_no_flags_writes_no_token(self):
        line = self._dry()
        self.assertIn("WOULD FILE Claude: Do x |", line)

    def test_the_token_reaches_the_creating_call(self):
        rec = self.install([self.CREATED])
        li.create_issue(
            "k",
            team_id="t",
            project_id="p",
            title="Do x",
            body="b",
            verb="architect",
            substrate="bedrock",
        )
        self.assertEqual(rec.calls[0][1]["input"]["title"], "[Architect/Bedrock] Do x")

    def test_refusals_write_nothing(self):
        cases = {
            "operator with a substrate": {"verb": "operator", "substrate": "bedrock"},
            "the silent default": {"verb": "task", "substrate": "bedrock"},
            "verb alone": {"verb": "chore"},
            "substrate alone": {"substrate": "anthropic"},
            "unknown verb": {"verb": "plan", "substrate": "bedrock"},
            "unknown substrate": {"verb": "chore", "substrate": "vertex"},
        }
        for name, token in cases.items():
            with self.subTest(name), self.assertRaises(li.LinearIssueError):
                self._dry(**token)

    def test_the_default_refusal_says_to_omit_both_flags(self):
        with self.assertRaises(li.LinearIssueError) as ctx:
            self._dry(verb="Task", substrate="BEDROCK")
        self.assertIn("omit both", str(ctx.exception))

    def test_the_cli_wires_verb_and_substrate(self):
        args = li._parse_args(
            [
                "prog",
                "create",
                "--title",
                "t",
                "--body-file",
                "f",
                "--verb",
                "chore",
                "--substrate",
                "anthropic",
            ]
        )
        self.assertEqual((args.verb, args.substrate), ("chore", "anthropic"))


class TickBoxes(unittest.TestCase):
    """The pure resolution: which lines a box text ticks, or why it refuses."""

    BODY = "\n".join(
        [
            "## Parts",
            "- [ ] Add the re-run block",
            "- [ ] Wire the probe",
            "- [x] Already landed",
            "* [ ] Fold the levers from [ENG-1986](https://linear.app/d/issue/ENG-1986/x)",
        ]
    )

    def test_an_exact_box_is_ticked_and_nothing_else_moves(self):
        body, ticked, already = li.tick_boxes(self.BODY, ["Wire the probe"])
        self.assertEqual((ticked, already), (1, 0))
        self.assertIn("- [x] Wire the probe", body)
        self.assertIn("- [ ] Add the re-run block", body)

    def test_a_tag_bearing_box_matches_on_the_bare_tag(self):
        # Linear stores the tag as a link; the stored markup is kept as is.
        body, ticked, _ = li.tick_boxes(self.BODY, ["Fold the levers from ENG-1986"])
        self.assertEqual(ticked, 1)
        self.assertIn("* [x] Fold the levers from [ENG-1986](", body)

    def test_a_unique_prefix_matches(self):
        _, ticked, _ = li.tick_boxes(self.BODY, ["Add the re-run"])
        self.assertEqual(ticked, 1)

    def test_an_already_ticked_box_is_counted_not_rewritten(self):
        body, ticked, already = li.tick_boxes(self.BODY, ["Already landed"])
        self.assertEqual((body, ticked, already), (self.BODY, 0, 1))

    def test_an_unknown_box_refuses(self):
        with self.assertRaisesRegex(li.LinearIssueError, "no checklist box"):
            li.tick_boxes(self.BODY, ["Not a box"])

    def test_duplicate_boxes_need_one_mention_each(self):
        body = "- [ ] Run lint\n## B\n- [ ] Run lint\n"
        with self.assertRaisesRegex(li.LinearIssueError, "2 box"):
            li.tick_boxes(body, ["Run lint"])
        new, ticked, _ = li.tick_boxes(body, ["Run lint", "Run lint"])
        self.assertEqual(ticked, 2)
        self.assertNotIn("[ ]", new)

    def test_a_prefix_never_lands_on_a_longer_box_past_a_ticked_one(self):
        # `Part 1` is ticked already; a prefix match must not fall through to
        # the open `Part 12`.
        body = "- [x] Part 1: foo\n- [ ] Part 12: bar\n"
        with self.assertRaisesRegex(li.LinearIssueError, "2 box"):
            li.tick_boxes(body, ["Part 1"])

    def test_two_texts_may_not_claim_one_box(self):
        with self.assertRaisesRegex(li.LinearIssueError, "both name one box"):
            li.tick_boxes(self.BODY, ["Wire the probe", "Wire the"])

    def test_naming_a_box_more_times_than_it_exists_refuses(self):
        with self.assertRaisesRegex(li.LinearIssueError, "named 2 time"):
            li.tick_boxes(self.BODY, ["Wire the probe", "Wire the probe"])

    def test_a_pasted_marker_in_any_form_is_stripped(self):
        for pasted in (
            "- [ ] Wire the probe",
            "* [X] Wire the probe",
            "  - [x] Wire the probe",
        ):
            _, ticked, _ = li.tick_boxes(self.BODY, [pasted])
            self.assertEqual(ticked, 1, pasted)

    def test_a_refusal_ticks_nothing(self):
        # Every box resolves before any is ticked: no half-applied body.
        with self.assertRaises(li.LinearIssueError):
            li.tick_boxes(self.BODY, ["Wire the probe", "Not a box"])


class Tick(_Stubbed):
    @staticmethod
    def issue(description="- [ ] One\n- [ ] Two", state=("Backlog", "unstarted")):
        return {
            "issue": {
                "id": "u1",
                "identifier": "ENG-1",
                "description": description,
                "state": {"name": state[0], "type": state[1]},
                "team": {
                    "states": {
                        "nodes": [
                            {"id": "s-ip", "name": "In Progress"},
                            {"id": "s-ir", "name": "In Review"},
                        ]
                    }
                },
            }
        }

    @staticmethod
    def ok(success=True):
        return {"issueUpdate": {"success": success}}

    def test_ticks_and_state_ride_one_write(self):
        recorder = self.install([self.issue(), self.ok()])
        line = li.tick("k", "ENG-1", ["One"])
        update = recorder.calls[-1][1]["input"]
        self.assertEqual(update["stateId"], "s-ip")
        self.assertEqual(update["description"], "- [x] One\n- [ ] Two")
        self.assertEqual(len(recorder.calls), 2)
        self.assertIn("TICKED ENG-1", line)

    def test_the_body_never_reaches_the_returned_line(self):
        self.install([self.issue("- [ ] One\nSECRET BODY"), self.ok()])
        self.assertNotIn("SECRET", li.tick("k", "ENG-1", ["One"]))

    def test_in_review_is_reclaimed(self):
        recorder = self.install([self.issue(state=("In Review", "started")), self.ok()])
        li.tick("k", "ENG-1", [])
        self.assertEqual(recorder.calls[-1][1]["input"], {"stateId": "s-ip"})

    def test_a_terminal_state_is_never_regressed(self):
        recorder = self.install([self.issue(state=("Done", "completed")), self.ok()])
        line = li.tick("k", "ENG-1", ["One"])
        self.assertNotIn("stateId", recorder.calls[-1][1]["input"])
        self.assertIn("terminal", line)

    def test_canceled_is_terminal_too(self):
        recorder = self.install([self.issue(state=("Canceled", "canceled"))])
        line = li.tick("k", "ENG-1", [])
        self.assertEqual(len(recorder.calls), 1)
        self.assertIn("terminal", line)

    def test_the_cli_wires_repeated_boxes_and_keep_state(self):
        recorder = self.install([self.issue(), self.ok()])
        real = li.linear_api.env_var
        self.addCleanup(lambda: setattr(li.linear_api, "env_var", real))
        li.linear_api.env_var = lambda *_a, **_k: "k"
        out = io.StringIO()
        with redirect_stdout(out):
            code = li.run(
                [
                    "linear_issue.py",
                    "tick",
                    "--id",
                    "ENG-1",
                    "--box",
                    "One",
                    "--box",
                    "Two",
                    "--keep-state",
                ]
            )
        self.assertEqual(code, 0)
        update = recorder.calls[-1][1]["input"]
        self.assertEqual(update, {"description": "- [x] One\n- [x] Two"})
        self.assertIn("2 box(es) ticked", out.getvalue())

    def test_the_cli_reads_code_span_boxes_from_a_box_file(self):
        """A code span in a box is refused inline by the shell guard."""
        body = "- [ ] `--since` day mode\n- [ ] Two\n- [ ] Three"
        recorder = self.install([self.issue(body), self.ok()])
        real = li.linear_api.env_var
        self.addCleanup(lambda: setattr(li.linear_api, "env_var", real))
        li.linear_api.env_var = lambda *_a, **_k: "k"
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "boxes.txt")
            with open(path, "w", encoding="utf-8") as fh:
                fh.write("`--since` day mode\n\n")
            argv = ["linear_issue.py", "tick", "--id", "ENG-1", "--box", "Two"]
            with redirect_stdout(io.StringIO()):
                code = li.run([*argv, "--box-file", path, "--keep-state"])
        self.assertEqual(code, 0)
        self.assertEqual(
            recorder.calls[-1][1]["input"]["description"],
            "- [x] `--since` day mode\n- [x] Two\n- [ ] Three",
        )

    def test_nothing_to_tick_and_already_in_progress_writes_nothing(self):
        recorder = self.install([self.issue(state=("In Progress", "started"))])
        line = li.tick("k", "ENG-1", [])
        self.assertEqual(len(recorder.calls), 1)
        self.assertIn("NOTHING TO WRITE", line)

    def test_a_failed_write_re_fetches_and_retries_once(self):
        recorder = self.install(
            [
                self.issue(),
                self.ok(success=False),
                # The second read reflects an edit made in between; the retry
                # rebuilds from it rather than the stale snapshot.
                self.issue("- [ ] One\n- [ ] Two\n- [ ] Three"),
                self.ok(),
            ]
        )
        line = li.tick("k", "ENG-1", ["One"])
        self.assertIn("retried once", line)
        self.assertIn("Three", recorder.calls[-1][1]["input"]["description"])

    def test_a_second_failure_stops(self):
        self.install([self.issue(), self.ok(False), self.issue(), self.ok(False)])
        with self.assertRaisesRegex(li.LinearIssueError, "leaving ENG-1 alone"):
            li.tick("k", "ENG-1", ["One"])

    def test_a_resolution_refusal_is_not_retried(self):
        recorder = self.install([self.issue()])
        with self.assertRaises(li.LinearIssueError):
            li.tick("k", "ENG-1", ["Missing"])
        self.assertEqual(len(recorder.calls), 1)

    def test_dry_run_reads_but_never_writes(self):
        recorder = self.install([self.issue()])
        line = li.tick("k", "ENG-1", ["One"], dry_run=True)
        self.assertEqual(len(recorder.calls), 1)
        self.assertIn("WOULD TICK", line)

    def test_keep_state_writes_no_state(self):
        recorder = self.install([self.issue(), self.ok()])
        li.tick("k", "ENG-1", ["One"], state=None)
        self.assertNotIn("stateId", recorder.calls[-1][1]["input"])


if __name__ == "__main__":
    unittest.main()
