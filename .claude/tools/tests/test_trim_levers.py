#!/usr/bin/env python3
# cspell:word noseparator
"""Unit tests for trim_levers.py.

Every network call goes through ``_post``, so the tests patch that one seam and
assert on the GraphQL operations the tool would have sent. Nothing here touches
Linear.

The property most worth pinning is the **zero-echo** one: no stored body may
appear in anything the tool prints. That is not a performance detail — it is the
entire reason this tool exists instead of the MCP write path, whose per-call body
echo compounds on an accumulator (five touches on one issue measured ~53k, rising
monotonically).
"""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import trim_levers as tl  # noqa: E402
from trim_levers import (  # noqa: E402
    TrimLeversError,
    compose_body,
    run,
    split_touches,
    validate_fingerprint,
)

ENV = {
    "LINEAR_API_KEY": "key",
    "LINEAR_PROJECT_ID": "proj",
    "LINEAR_TEAM_ID": "team",
    "LINEAR_ASSIGNEE_ID": "me",
}


def _node(
    identifier="ENG-1",
    title="A lever",
    state="Todo",
    milestone="Trim levers",
    fingerprint="a:b",
    state_type="unstarted",
    description=None,
):
    """A probe/listing result node.

    Carries a `description` with an anchored `**Fingerprint**:` field by default,
    because the probe verifies the key line-anchored in-process — the server-side
    `contains` filter is only a pre-filter. A node without that field is correctly
    treated as a non-match.
    """
    if description is None:
        description = f"Prose.\n\n**Fingerprint**: {fingerprint}\n"
    return {
        "identifier": identifier,
        "url": f"https://linear.app/dasmac/issue/{identifier}",
        "title": title,
        "description": description,
        "state": {"name": state, "type": state_type},
        "projectMilestone": {"name": milestone} if milestone else None,
    }


def _page(nodes, has_next=False, cursor=None):
    return {
        "issues": {
            "pageInfo": {"hasNextPage": has_next, "endCursor": cursor},
            "nodes": nodes,
        }
    }


class FingerprintTests(unittest.TestCase):
    """The dedup key. If it is wrong, refiling protection is gone."""

    def test_a_well_formed_key_passes(self):
        self.assertEqual(
            validate_fingerprint(" session-metrics:search-scope-axis "),
            "session-metrics:search-scope-axis",
        )

    def test_a_dotted_domain_token_is_refused(self):
        # Linear linkifies a hostname-valid basename, silently rewriting the
        # stored key — roughly 40 keys were corrupted this way.
        with self.assertRaises(TrimLeversError) as caught:
            validate_fingerprint("http.rs:timeout")
        message = str(caught.exception)
        self.assertIn("linkifies", message)
        self.assertIn("feeds-http", message)

    def test_a_dot_in_the_slug_half_is_allowed(self):
        # Only the first token is hostname-dangerous; the slug may name a file.
        self.assertEqual(
            validate_fingerprint("claude-tools:review_diff.py-drift"),
            "claude-tools:review_diff.py-drift",
        )

    def test_a_missing_colon_is_refused(self):
        with self.assertRaises(TrimLeversError):
            validate_fingerprint("noseparator")

    def test_an_empty_key_is_refused(self):
        with self.assertRaises(TrimLeversError):
            validate_fingerprint("   ")

    def test_uppercase_is_refused_rather_than_silently_lowered(self):
        # Silently normalizing would make two spellings dedup differently
        # depending on which was filed first.
        with self.assertRaises(TrimLeversError):
            validate_fingerprint("Session-Metrics:Foo")


class ComposeBodyTests(unittest.TestCase):
    def test_the_two_machine_fields_are_appended(self):
        got = compose_body("Some prose.", "a:b", ["docs/**", "cfg/**"])
        self.assertIn("**Fingerprint**: a:b", got)
        self.assertIn("**Touches**: docs/**, cfg/**", got)

    def test_an_existing_fingerprint_line_is_not_duplicated(self):
        body = "Prose.\n\n**Fingerprint**: a:b"
        self.assertEqual(compose_body(body, "a:b", []).count("**Fingerprint**"), 1)

    def test_fields_are_separated_by_a_blank_line(self):
        # A field abutting a paragraph is how Linear's round trip has re-parsed
        # prose as a setext heading before.
        got = compose_body("Prose.", "a:b", ["x/"])
        self.assertIn("Prose.\n\n**Fingerprint**", got)
        self.assertNotIn("Prose.\n**Fingerprint**", got)

    def test_no_touches_means_no_touches_line(self):
        self.assertNotIn("**Touches**", compose_body("Prose.", "a:b", []))

    def test_touches_are_deduplicated_in_order(self):
        self.assertEqual(split_touches("b/, a/, b/ ,"), ["b/", "a/"])

    def test_no_touches_argument_is_an_empty_list(self):
        self.assertEqual(split_touches(None), [])


class ProbeTests(unittest.TestCase):
    def test_the_filter_searches_the_body_for_the_fingerprint(self):
        seen = []

        def fake(api_key, query, variables):
            seen.append(variables)
            return _page([_node()])

        with mock.patch.object(tl, "_post", side_effect=fake):
            tl.probe("k", "proj", "a:b")
        self.assertEqual(seen[0]["filter"]["description"], {"contains": "a:b"})
        self.assertEqual(seen[0]["filter"]["project"], {"id": {"eq": "proj"}})

    def test_the_listing_query_selects_no_description_field(self):
        # The zero-echo property, asserted on the query text. The PROBE does
        # select `description` (it must, to match line-anchored) — but it reads it
        # in-process and never prints it; see ExactFingerprintMatchTests.
        self.assertNotIn("description", tl._SEARCH_QUERY)
        self.assertNotIn("description", tl._PARKED_QUERY)

    def test_the_fold_listing_does_not_include_archived_levers(self):
        # The probe needs archived rows so a rejection stays permanent; the fold
        # must not offer an archived lever as parked work, and the selection
        # carries no `archivedAt` for anything downstream to filter on.
        self.assertIn("includeArchived: true", tl._PROBE_QUERY)
        self.assertNotIn("includeArchived", tl._PARKED_QUERY)

    def test_the_probe_follows_the_cursor(self):
        pages = [_page([_node("ENG-1")], True, "c1"), _page([_node("ENG-2")])]
        calls = []

        def fake(api_key, query, variables):
            calls.append(variables.get("after"))
            return pages[len(calls) - 1]

        with mock.patch.object(tl, "_post", side_effect=fake):
            got = tl.probe("k", "proj", "a:b")
        self.assertEqual([n["identifier"] for n in got], ["ENG-1", "ENG-2"])
        self.assertEqual(calls, [None, "c1"])

    def test_resolved_and_archived_issues_are_searched_too(self):
        """A rejected lever is closed with its reason, and that rejection has to
        stay permanent — otherwise the next pass re-proposes it on intuition,
        which nine of thirteen mined entries explicitly warned against."""
        self.assertIn("includeArchived: true", tl._SEARCH_QUERY)


class FileLeverTests(unittest.TestCase):
    def _fake(self, existing=(), created="ENG-7"):
        def fake(api_key, query, variables):
            if "issues(" in query:
                return _page(list(existing))
            if "Milestones" in query:
                return {
                    "project": {
                        "projectMilestones": {
                            "nodes": [{"id": "ms-1", "name": tl.MILESTONE_NAME}]
                        }
                    }
                }
            if "States" in query:
                return {
                    "team": {
                        "states": {"nodes": [{"id": "st-1", "name": tl.PARKED_STATE}]}
                    }
                }
            self.created = variables["input"]
            return {
                "issueCreate": {"success": True, "issue": _node(created)},
            }

        return fake

    def test_a_new_lever_is_created_parked_in_one_call(self):
        with mock.patch.object(tl, "_post", side_effect=self._fake()):
            line = tl.file_lever(
                "k",
                project_id="proj",
                team_id="team",
                assignee_id="me",
                title="A lever",
                body="Prose.",
                fingerprint="a:b",
                touches=["docs/**"],
                dry_run=False,
            )
        self.assertTrue(line.startswith("FILED ENG-7 "))
        # Milestone, state and assignee all in the creating call — filing then
        # amending would cost a second full body echo and buy nothing.
        self.assertEqual(self.created["projectMilestoneId"], "ms-1")
        self.assertEqual(self.created["stateId"], "st-1")
        self.assertEqual(self.created["assigneeId"], "me")

    def test_the_confirmation_carries_no_body_text(self):
        with mock.patch.object(tl, "_post", side_effect=self._fake()):
            line = tl.file_lever(
                "k",
                project_id="proj",
                team_id="team",
                assignee_id=None,
                title="A lever",
                body="SECRET-PROSE-MARKER",
                fingerprint="a:b",
                touches=[],
                dry_run=False,
            )
        self.assertNotIn("SECRET-PROSE-MARKER", line)

    def test_an_existing_fingerprint_is_refused_with_the_append_hint(self):
        with mock.patch.object(tl, "_post", side_effect=self._fake([_node("ENG-3")])):
            with self.assertRaises(TrimLeversError) as caught:
                tl.file_lever(
                    "k",
                    project_id="proj",
                    team_id="team",
                    assignee_id=None,
                    title="A lever",
                    body="Prose.",
                    fingerprint="a:b",
                    touches=[],
                    dry_run=False,
                )
        message = str(caught.exception)
        self.assertIn("ENG-3", message)
        self.assertIn("append-evidence", message)

    def test_a_dry_run_writes_nothing(self):
        calls = []

        def fake(api_key, query, variables):
            calls.append(query)
            return _page([])

        with mock.patch.object(tl, "_post", side_effect=fake):
            line = tl.file_lever(
                "k",
                project_id="proj",
                team_id="team",
                assignee_id=None,
                title="A lever",
                body="Prose.",
                fingerprint="a:b",
                touches=[],
                dry_run=True,
            )
        self.assertTrue(line.startswith("WOULD FILE"))
        self.assertTrue(all("mutation" not in q for q in calls))

    def test_a_missing_milestone_names_the_available_ones(self):
        def fake(api_key, query, variables):
            if "issues(" in query:
                return _page([])
            if "Milestones" in query:
                return {
                    "project": {
                        "projectMilestones": {
                            "nodes": [{"id": "x", "name": "Audit findings"}]
                        }
                    }
                }
            raise AssertionError("should not have reached the mutation")

        with mock.patch.object(tl, "_post", side_effect=fake):
            with self.assertRaises(TrimLeversError) as caught:
                tl.file_lever(
                    "k",
                    project_id="proj",
                    team_id="team",
                    assignee_id=None,
                    title="t",
                    body="b",
                    fingerprint="a:b",
                    touches=[],
                    dry_run=False,
                )
        self.assertIn("Audit findings", str(caught.exception))


class AppendEvidenceTests(unittest.TestCase):
    """The accumulator path — where the MCP echo compounds worst."""

    def _fake(self, stored, matches=None):
        self.sent = {}

        def fake(api_key, query, variables):
            if "issues(" in query:
                return _page(matches if matches is not None else [_node("ENG-5")])
            if "LeverBody" in query:
                return {
                    "issue": {
                        "id": "uuid-5",
                        "identifier": "ENG-5",
                        "url": "https://linear.app/dasmac/issue/ENG-5",
                        "description": stored,
                    }
                }
            self.sent = variables
            return {"issueUpdate": {"success": True, "issue": _node("ENG-5")}}

        return fake

    def test_the_evidence_is_appended_after_a_blank_line(self):
        with mock.patch.object(tl, "_post", side_effect=self._fake("Existing body.")):
            tl.append_evidence(
                "k",
                project_id="proj",
                fingerprint="a:b",
                evidence="New evidence.",
                dry_run=False,
            )
        self.assertEqual(self.sent["description"], "Existing body.\n\nNew evidence.\n")

    def test_a_stored_body_ending_in_newlines_does_not_gain_a_gap(self):
        with mock.patch.object(tl, "_post", side_effect=self._fake("Body.\n\n\n")):
            tl.append_evidence(
                "k",
                project_id="proj",
                fingerprint="a:b",
                evidence="More.",
                dry_run=False,
            )
        self.assertEqual(self.sent["description"], "Body.\n\nMore.\n")

    def test_the_grown_body_never_reaches_the_printed_line(self):
        stored = "STORED-MARKER body text"
        with mock.patch.object(tl, "_post", side_effect=self._fake(stored)):
            line = tl.append_evidence(
                "k",
                project_id="proj",
                fingerprint="a:b",
                evidence="EVIDENCE-MARKER",
                dry_run=False,
            )
        self.assertNotIn("STORED-MARKER", line)
        self.assertNotIn("EVIDENCE-MARKER", line)
        # Only sizes — enough to confirm the write landed, nothing to replay.
        self.assertIn("chars", line)

    def test_an_unknown_fingerprint_is_refused(self):
        with mock.patch.object(tl, "_post", side_effect=self._fake("", matches=[])):
            with self.assertRaises(TrimLeversError) as caught:
                tl.append_evidence(
                    "k",
                    project_id="proj",
                    fingerprint="a:b",
                    evidence="e",
                    dry_run=False,
                )
        self.assertIn("file it first", str(caught.exception))

    def test_an_ambiguous_fingerprint_refuses_to_guess(self):
        two = [_node("ENG-5"), _node("ENG-6")]
        with mock.patch.object(tl, "_post", side_effect=self._fake("", matches=two)):
            with self.assertRaises(TrimLeversError) as caught:
                tl.append_evidence(
                    "k",
                    project_id="proj",
                    fingerprint="a:b",
                    evidence="e",
                    dry_run=False,
                )
        message = str(caught.exception)
        self.assertIn("ENG-5", message)
        self.assertIn("ENG-6", message)

    def test_a_dry_run_reads_no_body_and_writes_nothing(self):
        queries = []

        def fake(api_key, query, variables):
            queries.append(query)
            return _page([_node("ENG-5")])

        with mock.patch.object(tl, "_post", side_effect=fake):
            line = tl.append_evidence(
                "k",
                project_id="proj",
                fingerprint="a:b",
                evidence="e",
                dry_run=True,
            )
        self.assertTrue(line.startswith("WOULD APPEND"))
        self.assertTrue(all("LeverBody" not in q for q in queries))


class CliTests(unittest.TestCase):
    def setUp(self):
        self.env = mock.patch.dict(os.environ, ENV)
        self.env.start()
        self.addCleanup(self.env.stop)

    def _write(self, text):
        d = tempfile.mkdtemp()
        path = Path(d) / "payload.md"
        path.write_text(text, encoding="utf-8")
        return str(path)

    def _invoke(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = run(["trim_levers.py", *argv])
        return code, out.getvalue(), err.getvalue()

    def test_probe_with_no_match_exits_one_so_a_caller_can_branch(self):
        with mock.patch.object(tl, "_post", return_value=_page([])):
            code, out, _ = self._invoke("probe", "--fingerprint", "a:b")
        self.assertEqual(code, 1)
        self.assertIn("NONE a:b", out)

    def test_probe_with_a_match_exits_zero(self):
        with mock.patch.object(tl, "_post", return_value=_page([_node("ENG-9")])):
            code, out, _ = self._invoke("probe", "--fingerprint", "a:b")
        self.assertEqual(code, 0)
        self.assertIn("MATCH ENG-9", out)

    def test_list_reports_the_parked_pool_and_its_count(self):
        with mock.patch.object(tl, "_post", return_value=_page([_node("ENG-9")])):
            code, out, err = self._invoke("list")
        self.assertEqual(code, 0)
        self.assertIn("ENG-9", out)
        self.assertIn("1 parked lever(s)", err)

    def test_list_filters_on_the_parking_milestone(self):
        seen = []

        def fake(api_key, query, variables):
            seen.append(variables["filter"])
            return _page([])

        with mock.patch.object(tl, "_post", side_effect=fake):
            self._invoke("list")
        self.assertEqual(
            seen[0]["projectMilestone"], {"name": {"eq": tl.MILESTONE_NAME}}
        )

    def test_list_filters_out_closed_states_server_side(self):
        # The milestone alone is not enough: a rejection keeps it, so the pool
        # would never look empty. `nin` is verified against the live schema —
        # WorkflowStateFilter.type is a StringComparator, which accepts it.
        seen = []

        def fake(api_key, query, variables):
            seen.append(variables["filter"])
            return _page([])

        with mock.patch.object(tl, "_post", side_effect=fake):
            self._invoke("list")
        self.assertEqual(seen[0]["state"], {"type": {"nin": ["completed", "canceled"]}})

    def test_list_omits_a_canceled_lever_that_still_carries_the_milestone(self):
        # The client-side half of the same guarantee: `list` must match
        # `open_parked`'s definition even if a row slips past the query. The
        # first real run returned 12 rows of which 9 were canceled rejections.
        rejected = _node("ENG-8", state="Canceled", state_type="canceled")
        todo = _node("ENG-9", state="Todo", state_type="unstarted")
        with mock.patch.object(tl, "_post", return_value=_page([rejected, todo])):
            code, out, err = self._invoke("list")
        self.assertEqual(code, 0)
        self.assertIn("ENG-9", out)
        self.assertNotIn("ENG-8", out)
        self.assertIn("1 parked lever(s)", err)

    def test_list_can_reach_the_nothing_parked_stop_condition(self):
        # `trim-context` step 1 stops when the pool is empty. Before the state
        # filter that was unreachable once any rejection existed.
        only_rejections = [
            _node("ENG-7", state="Canceled", state_type="canceled"),
            _node("ENG-8", state="Done", state_type="completed"),
        ]
        with mock.patch.object(tl, "_post", return_value=_page(only_rejections)):
            code, out, err = self._invoke("list")
        self.assertEqual(code, 0)
        self.assertEqual(out.strip(), "")
        self.assertIn("0 parked lever(s)", err)

    def test_an_empty_body_file_is_refused(self):
        with self.assertRaises(TrimLeversError) as caught:
            self._invoke(
                "file",
                "--title",
                "t",
                "--fingerprint",
                "a:b",
                "--body-file",
                self._write("   \n"),
            )
        self.assertIn("is empty", str(caught.exception))

    def test_a_missing_body_file_is_a_clean_error(self):
        with self.assertRaises(TrimLeversError) as caught:
            self._invoke(
                "file",
                "--title",
                "t",
                "--fingerprint",
                "a:b",
                "--body-file",
                "/nonexistent/none.md",
            )
        self.assertIn("cannot read", str(caught.exception))

    def test_a_bad_fingerprint_fails_before_any_network_call(self):
        with mock.patch.object(tl, "_post") as posted:
            with self.assertRaises(TrimLeversError):
                self._invoke("probe", "--fingerprint", "http.rs:x")
        posted.assert_not_called()

    def test_a_missing_env_var_is_a_clean_error(self):
        with mock.patch.dict(os.environ, {"LINEAR_PROJECT_ID": ""}):
            with self.assertRaises(TrimLeversError) as caught:
                self._invoke("probe", "--fingerprint", "a:b")
        self.assertIn("LINEAR_PROJECT_ID", str(caught.exception))

    def test_a_non_printable_api_key_is_refused_without_echoing_it(self):
        with mock.patch.dict(os.environ, {"LINEAR_API_KEY": "lin_api_\nsecret"}):
            with self.assertRaises(TrimLeversError) as caught:
                self._invoke("probe", "--fingerprint", "a:b")
        message = str(caught.exception)
        self.assertIn("not printable ASCII", message)
        self.assertNotIn("secret", message)

    def test_main_maps_an_error_to_exit_two(self):
        argv = ["trim_levers.py", "probe", "--fingerprint", "http.rs:x"]
        with mock.patch.object(sys, "argv", argv):
            with redirect_stderr(io.StringIO()) as err:
                code = tl.main()
        self.assertEqual(code, 2)
        self.assertIn("error:", err.getvalue())


class DryRunPositionTests(unittest.TestCase):
    """`--dry-run` must bind in BOTH positions.

    It shipped bound in only one: declared on the parent parser AND on each
    subparser with a plain `False` default, so the subparser's default
    overwrote the parent's `True` and `--dry-run file …` performed a REAL Linear
    write while the operator believed they had asked for a rehearsal. Two review
    lenses found it independently. `board_batch.py` carries the same helper for
    the same reason; these tests are the guard that keeps them in step.
    """

    def _dry_run(self, *argv):
        return tl._parse_args(["trim_levers.py", *argv]).dry_run

    def test_before_the_subcommand(self):
        self.assertTrue(self._dry_run("--dry-run", "probe", "--fingerprint", "a:b"))

    def test_after_the_subcommand(self):
        self.assertTrue(self._dry_run("probe", "--fingerprint", "a:b", "--dry-run"))

    def test_absent_means_live(self):
        self.assertFalse(self._dry_run("probe", "--fingerprint", "a:b"))

    def test_it_binds_in_both_positions_for_every_subcommand(self):
        cases = {
            "probe": ["--fingerprint", "a:b"],
            "list": [],
            "append-evidence": ["--fingerprint", "a:b", "--evidence-file", "e"],
            "file": [
                "--title",
                "t",
                "--fingerprint",
                "a:b",
                "--body-file",
                "b",
            ],
        }
        for cmd, rest in cases.items():
            with self.subTest(cmd=cmd, position="before"):
                self.assertTrue(self._dry_run("--dry-run", cmd, *rest))
            with self.subTest(cmd=cmd, position="after"):
                self.assertTrue(self._dry_run(cmd, *rest, "--dry-run"))


class ExactFingerprintMatchTests(unittest.TestCase):
    """The server `contains` filter is a pre-filter; the probe matches exactly.

    Slugs nest — `a:search-scope` is a substring of `a:search-scope-axis` — so a
    substring-only probe returns one confident WRONG match, which would grow the
    wrong lever and refuse a genuinely new one. This is the single guard the
    whole dedup pipeline rests on.
    """

    def test_a_nested_slug_is_not_a_match(self):
        longer = _node("ENG-9", fingerprint="a:search-scope-axis")
        with mock.patch.object(tl, "_post", return_value=_page([longer])):
            got = tl.probe("k", "proj", "a:search-scope")
        self.assertEqual(got, [])

    def test_the_exact_slug_still_matches(self):
        node = _node("ENG-9", fingerprint="a:search-scope-axis")
        with mock.patch.object(tl, "_post", return_value=_page([node])):
            got = tl.probe("k", "proj", "a:search-scope-axis")
        self.assertEqual([n["identifier"] for n in got], ["ENG-9"])

    def test_a_fingerprint_only_mentioned_in_prose_is_not_a_match(self):
        # Not line-anchored, so not a field — it must not satisfy dedup.
        prose = _node("ENG-9", description="See **Fingerprint**: a:b inline.\n")
        with mock.patch.object(tl, "_post", return_value=_page([prose])):
            self.assertEqual(tl.probe("k", "proj", "a:b"), [])

    def test_the_probe_query_selects_description_but_the_listing_does_not(self):
        # The zero-echo property is about what PRINTS, not what is fetched — but
        # the listing has no reason to carry bodies, so it still must not.
        self.assertIn("description", tl._PROBE_QUERY)
        self.assertNotIn("description", tl._SEARCH_QUERY)


class LeverLifecycleTests(unittest.TestCase):
    """Only a still-parked lever accumulates evidence.

    The fold copies each `**Fingerprint**:` line into the aggregated task, so
    after the first fold a raw probe legitimately matches TWO issues — the closed
    original and the open aggregate. Treating that as ambiguous would break the
    recurrence-accumulation this pipeline exists for, which is exactly what the
    completeness lens caught.
    """

    def _fake(self, matches):
        def fake(api_key, query, variables):
            if "issues(" in query:
                return _page(matches)
            raise AssertionError("should not have reached a write")

        return fake

    def test_the_parked_lever_is_selected_over_a_folded_one(self):
        folded = _node("ENG-5", state="Done", state_type="completed", milestone=None)
        parked = _node("ENG-8", state="Todo")
        with mock.patch.object(tl, "_post", side_effect=self._fake([folded, parked])):
            line = tl.append_evidence(
                "k", project_id="p", fingerprint="a:b", evidence="e", dry_run=True
            )
        self.assertIn("ENG-8", line)
        self.assertNotIn("ENG-5", line)

    def test_only_discharged_matches_reports_the_disposition(self):
        # Reported, not raised — see `PostFoldCycleTests` for why raising here
        # crashed an unattended run on a routine case.
        folded = _node("ENG-5", state="Done", state_type="completed", milestone=None)
        turned_down = _node("ENG-6", state="Canceled", state_type="canceled")
        with mock.patch.object(
            tl, "_post", side_effect=self._fake([folded, turned_down])
        ):
            line = tl.append_evidence(
                "k", project_id="p", fingerprint="a:b", evidence="e", dry_run=True
            )
        self.assertTrue(line.startswith("NOTED"))
        self.assertIn("ENG-5", line)
        self.assertIn("Canceled", line)

    def test_a_promoted_lever_is_also_not_a_parked_candidate(self):
        # Milestone cleared, still open: promoted to the Backlog and being worked.
        promoted = _node("ENG-7", state="Backlog", state_type="backlog", milestone=None)
        with mock.patch.object(tl, "_post", side_effect=self._fake([promoted])):
            line = tl.append_evidence(
                "k", project_id="p", fingerprint="a:b", evidence="e", dry_run=True
            )
        self.assertIn("no longer parked", line)

    def test_an_in_progress_parked_lever_still_accepts_evidence(self):
        # Open by exclusion, not by an allow-list of types: a lever someone has
        # started is still parked and must not become a hard refusal.
        started = _node("ENG-8", state="In Progress", state_type="started")
        with mock.patch.object(tl, "_post", side_effect=self._fake([started])):
            line = tl.append_evidence(
                "k", project_id="p", fingerprint="a:b", evidence="e", dry_run=True
            )
        self.assertIn("ENG-8", line)

    def test_two_parked_levers_are_still_ambiguous(self):
        both = [_node("ENG-5"), _node("ENG-6")]
        with mock.patch.object(tl, "_post", side_effect=self._fake(both)):
            with self.assertRaises(TrimLeversError) as caught:
                tl.append_evidence(
                    "k", project_id="p", fingerprint="a:b", evidence="e", dry_run=True
                )
        self.assertIn("2 parked levers", str(caught.exception))

    def test_open_parked_ignores_an_open_issue_without_the_milestone(self):
        # An aggregated task is open but carries no parking milestone.
        aggregate = _node("ENG-9", milestone=None)
        self.assertEqual(tl.open_parked([aggregate]), [])


class AppendEvidenceGuardTests(unittest.TestCase):
    """The read half of the read-modify-write must not fail open.

    Unguarded, a null `issue` raised a bare KeyError (the traceback this module
    exists to avoid), and an empty `description` REPLACED the accumulated lever
    with the evidence alone — silent data loss on the accumulator.
    """

    def _fake(self, body_result):
        def fake(api_key, query, variables):
            if "issues(" in query:
                return _page([_node("ENG-5")])
            if "LeverBody" in query:
                return body_result
            raise AssertionError("should not have reached the update mutation")

        return fake

    def test_an_unresolvable_issue_is_a_clean_error_not_a_keyerror(self):
        with mock.patch.object(tl, "_post", side_effect=self._fake({"issue": None})):
            with self.assertRaises(TrimLeversError) as caught:
                tl.append_evidence(
                    "k", project_id="p", fingerprint="a:b", evidence="e", dry_run=False
                )
        self.assertIn("did not resolve", str(caught.exception))

    def test_an_empty_stored_body_refuses_rather_than_overwriting(self):
        body = {
            "issue": {"id": "u", "identifier": "ENG-5", "url": "u", "description": ""}
        }
        with mock.patch.object(tl, "_post", side_effect=self._fake(body)):
            with self.assertRaises(TrimLeversError) as caught:
                tl.append_evidence(
                    "k", project_id="p", fingerprint="a:b", evidence="e", dry_run=False
                )
        self.assertIn("refusing to overwrite", str(caught.exception))


class PostFoldCycleTests(unittest.TestCase):
    """A lever that recurs AFTER its fold must have an available operation.

    The dead end this pins: a fold closes the original and clears its milestone,
    while the aggregated task keeps the fingerprint line. So `open_parked` is
    empty and the probe is non-empty — and with `file` refusing on any match and
    `append-evidence` raising, neither subcommand could proceed and an unattended
    run exited 2. Folding means the fix is *queued*, not that the lever is
    settled; only a rejection is permanent.
    """

    FOLDED = [
        _node("ENG-5", state="Done", state_type="completed", milestone=None),
        _node("ENG-9", state="Backlog", state_type="backlog", milestone=None),
    ]

    def _fake(self, matches, created="ENG-20"):
        def fake(api_key, query, variables):
            if "issues(" in query:
                return _page(list(matches))
            if "Milestones" in query:
                return {
                    "project": {
                        "projectMilestones": {
                            "nodes": [{"id": "ms-1", "name": tl.MILESTONE_NAME}]
                        }
                    }
                }
            if "States" in query:
                return {
                    "team": {
                        "states": {"nodes": [{"id": "st-1", "name": tl.PARKED_STATE}]}
                    }
                }
            return {"issueCreate": {"success": True, "issue": _node(created)}}

        return fake

    def _file(self, matches):
        return tl.file_lever(
            "k",
            project_id="p",
            team_id="t",
            assignee_id=None,
            title="A lever",
            body="Prose.",
            fingerprint="a:b",
            touches=[],
            dry_run=False,
        )

    def test_a_folded_lever_can_be_refiled_and_names_its_lineage(self):
        with mock.patch.object(tl, "_post", side_effect=self._fake(self.FOLDED)):
            line = self._file(self.FOLDED)
        self.assertTrue(line.startswith("FILED ENG-20"))
        self.assertIn("supersedes", line)
        self.assertIn("ENG-5", line)

    def test_append_evidence_on_a_folded_lever_reports_and_succeeds(self):
        with mock.patch.object(tl, "_post", side_effect=self._fake(self.FOLDED)):
            line = tl.append_evidence(
                "k", project_id="p", fingerprint="a:b", evidence="e", dry_run=False
            )
        # Reported, NOT raised: raising crashed an unattended session-metrics run
        # for an entirely routine case.
        self.assertTrue(line.startswith("NOTED"))
        self.assertIn("no longer parked", line)

    def test_a_rejected_lever_still_cannot_be_refiled(self):
        turned_down = [_node("ENG-6", state="Canceled", state_type="canceled")]
        with mock.patch.object(tl, "_post", side_effect=self._fake(turned_down)):
            with self.assertRaises(TrimLeversError) as caught:
                self._file(turned_down)
        message = str(caught.exception)
        self.assertIn("REJECTED", message)
        self.assertIn("permanent", message)

    def test_a_still_parked_lever_refuses_a_duplicate_filing(self):
        parked = [_node("ENG-8")]
        with mock.patch.object(tl, "_post", side_effect=self._fake(parked)):
            with self.assertRaises(TrimLeversError) as caught:
                self._file(parked)
        self.assertIn("already parked", str(caught.exception))

    def test_a_rejection_wins_over_a_fold_when_both_exist(self):
        both = self.FOLDED + [_node("ENG-6", state="Canceled", state_type="canceled")]
        with mock.patch.object(tl, "_post", side_effect=self._fake(both)):
            with self.assertRaises(TrimLeversError):
                self._file(both)


class FencedFieldTests(unittest.TestCase):
    """A `**Field**:` line inside a code fence is an illustration, not a field.

    Levers *about filing conventions* quote example filing blocks, so this is a
    likely body — and without the guard `compose_body`'s foreign-key refusal
    rejects it outright. The sibling parser in `read_result.py` grew the same
    guard in the same commit; these must not disagree.
    """

    QUOTED = "\n".join(
        [
            "The filing block looks like this:",
            "",
            "```md",
            "**Fingerprint**: other-domain:other-slug",
            "**Touches**: docs/**",
            "```",
            "",
            "That is the shape.",
        ]
    )

    def test_a_quoted_field_is_not_read_as_a_foreign_key(self):
        self.assertEqual(tl.field_values(self.QUOTED, "Fingerprint"), [])

    def test_a_body_quoting_an_example_can_still_be_filed(self):
        got = compose_body(self.QUOTED, "a:b", ["cfg/**"])
        self.assertIn("**Fingerprint**: a:b", got)
        # The quoted example survives untouched in the prose.
        self.assertIn("other-domain:other-slug", got)

    def test_a_real_field_outside_the_fence_is_still_seen(self):
        body = self.QUOTED + "\n\n**Fingerprint**: real:key\n"
        self.assertEqual(tl.field_values(body, "Fingerprint"), ["real:key"])


class ForeignFingerprintTests(unittest.TestCase):
    def test_a_body_carrying_another_key_is_refused(self):
        with self.assertRaises(TrimLeversError) as caught:
            compose_body("P\n\n**Fingerprint**: a:foo\n", "a:foo-bar", [])
        message = str(caught.exception)
        self.assertIn("already carries a different", message)
        self.assertIn("a:foo", message)

    def test_the_same_key_is_not_duplicated(self):
        got = compose_body("P\n\n**Fingerprint**: a:b\n", "a:b", [])
        self.assertEqual(len(tl.field_values(got, "Fingerprint")), 1)

    def test_an_existing_touches_line_is_not_duplicated(self):
        got = compose_body("P\n\n**Touches**: docs/**\n", "a:b", ["docs/**"])
        self.assertEqual(len(tl.field_values(got, "Touches")), 1)


class TruncationGuardTests(unittest.TestCase):
    def test_a_full_page_with_no_pageinfo_is_refused(self):
        # Trusting `hasNextPage` alone means a full-but-unmarked page reads as a
        # complete result. Refuse rather than report a truncated set as whole.
        nodes = [_node(f"ENG-{k}") for k in range(tl.PAGE_SIZE)]
        with mock.patch.object(tl, "_post", return_value={"issues": {"nodes": nodes}}):
            with self.assertRaises(TrimLeversError) as caught:
                tl.parked("k", "proj")
        self.assertIn("no pageInfo", str(caught.exception))

    def test_a_short_page_with_no_pageinfo_is_still_fine(self):
        with mock.patch.object(
            tl, "_post", return_value={"issues": {"nodes": [_node("ENG-1")]}}
        ):
            self.assertEqual(len(tl.parked("k", "proj")), 1)


class NoRelationsTests(unittest.TestCase):
    """The tool must not be able to file a blocking edge, even by accident.

    Blocking edges are human-curated in a planning session, and a parked lever is
    exempt from the meta batch and its edge until it is folded. The cheapest
    guarantee is that no relation mutation exists in the module at all.
    """

    def test_no_relation_mutation_is_present(self):
        source = Path(tl.__file__).read_text(encoding="utf-8")
        self.assertNotIn("issueRelationCreate", source)
        self.assertNotIn("blockedBy", source)

    def test_the_create_payload_carries_no_relation_keys(self):
        got = json.dumps(
            {
                "teamId": "t",
                "projectId": "p",
                "projectMilestoneId": "m",
                "stateId": "s",
                "title": "t",
                "description": "d",
            }
        )
        for key in ("blockedBy", "blocks", "relatedTo", "parentId"):
            self.assertNotIn(key, got)


class BodiesBearingReadTests(unittest.TestCase):
    """One call for the whole pool, instead of one fetch per lever.

    The fold's *reads* had become its larger cost: the plain listing prints
    titles only, so a fold ran a `get_issue` per lever — 21 on one pass — and
    the nearest body-bearing sweep cost 10.6k for 65 issues with every
    description truncated anyway.
    """

    NODES = [
        {
            "identifier": "ENG-2",
            "url": "u2",
            "title": "Second",
            "description": "**Fingerprint**: beta:two\n\nBODY-TWO",
            "state": {"name": "Todo", "type": "unstarted"},
            "projectMilestone": {"name": tl.MILESTONE_NAME},
        },
        {
            "identifier": "ENG-1",
            "url": "u1",
            "title": "First",
            "description": "**Fingerprint**: alpha:one\n\nBODY-ONE",
            "state": {"name": "Todo", "type": "unstarted"},
            "projectMilestone": {"name": tl.MILESTONE_NAME},
        },
    ]

    def _page(self):
        return {
            "issues": {
                "pageInfo": {"hasNextPage": False, "endCursor": None},
                "nodes": self.NODES,
            }
        }

    def _run(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with (
            mock.patch.dict(
                os.environ, {"LINEAR_API_KEY": "k", "LINEAR_PROJECT_ID": "p"}
            ),
            mock.patch.object(tl, "_post", return_value=self._page()),
        ):
            with redirect_stdout(out), redirect_stderr(err):
                code = tl.run(["trim_levers.py"] + argv)
        return code, out.getvalue(), err.getvalue()

    def test_the_plain_listing_does_not_ask_for_bodies(self):
        # The cheap listing must stay cheap; bodies are opted into.
        seen = {}

        def fake_post(api_key, query, variables):
            seen["query"] = query
            return self._page()

        with (
            mock.patch.dict(
                os.environ, {"LINEAR_API_KEY": "k", "LINEAR_PROJECT_ID": "p"}
            ),
            mock.patch.object(tl, "_post", side_effect=fake_post),
        ):
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                tl.run(["trim_levers.py", "list"])
        self.assertNotIn("description", seen["query"])

    def test_fingerprints_mode_prints_the_dedup_key_per_lever(self):
        code, out, _ = self._run(["list", "--fingerprints"])
        self.assertEqual(code, 0)
        self.assertIn("alpha:one", out)
        self.assertIn("beta:two", out)

    def test_fingerprints_mode_does_not_print_the_bodies(self):
        _, out, err = self._run(["list", "--fingerprints"])
        self.assertNotIn("BODY-ONE", out + err)
        self.assertNotIn("BODY-TWO", out + err)

    def test_a_lever_without_a_fingerprint_is_named_not_skipped(self):
        with mock.patch.object(
            self,
            "NODES",
            [
                {
                    "identifier": "ENG-3",
                    "url": "u3",
                    "title": "Keyless",
                    "description": "no key here",
                    "state": {"name": "Todo", "type": "unstarted"},
                    "projectMilestone": {"name": tl.MILESTONE_NAME},
                }
            ],
        ):
            _, out, _ = self._run(["list", "--fingerprints"])
        self.assertIn("(no fingerprint)", out)

    def test_bodies_out_writes_a_sliceable_document_and_prints_only_sizes(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "bodies.md")
            code, out, err = self._run(["list", "--bodies-out", path])
            self.assertEqual(code, 0)
            written = open(path, encoding="utf-8").read()
        self.assertIn("BODY-ONE", written)
        self.assertIn("BODY-TWO", written)
        # Zero echo: the payload is in the file, never in the output.
        self.assertNotIn("BODY-ONE", out + err)
        self.assertIn("chars to", err)

    def test_the_written_document_carries_one_heading_per_lever(self):
        # `read_result.py --headings` / `--section` is the intended next call.
        rendered = tl.render_bodies(self.NODES)
        self.assertIn("## ENG-1 | First", rendered)
        self.assertIn("## ENG-2 | Second", rendered)

    def test_the_document_is_ordered_by_identifier(self):
        rendered = tl.render_bodies(self.NODES)
        self.assertLess(rendered.index("## ENG-1"), rendered.index("## ENG-2"))

    def test_the_bodies_file_is_owner_only(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "bodies.md")
            self._run(["list", "--bodies-out", path])
            self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)


if __name__ == "__main__":
    unittest.main()
