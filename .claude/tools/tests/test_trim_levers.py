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


def _node(identifier="ENG-1", title="A lever", state="Todo", milestone="Trim levers"):
    return {
        "identifier": identifier,
        "url": f"https://linear.app/dasmac/issue/{identifier}",
        "title": title,
        "state": {"name": state, "type": "unstarted"},
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

    def test_the_probe_selects_no_description_field(self):
        # The zero-echo property, asserted on the query text itself.
        self.assertNotIn("description", tl._SEARCH_QUERY)

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
            if "query Levers" in query:
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
            if "query Levers" in query:
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
            if "query Levers" in query:
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
        self.assertIn("non-printable", message)
        self.assertNotIn("secret", message)

    def test_main_maps_an_error_to_exit_two(self):
        argv = ["trim_levers.py", "probe", "--fingerprint", "http.rs:x"]
        with mock.patch.object(sys, "argv", argv):
            with redirect_stderr(io.StringIO()) as err:
                code = tl.main()
        self.assertEqual(code, 2)
        self.assertIn("error:", err.getvalue())


class NoRelationsTests(unittest.TestCase):
    """The tool must not be able to file a blocking edge, even by accident.

    Blocking edges are human-curated in a planning session, and a parked lever is
    exempt from the serial meta chain until it is folded. The cheapest guarantee is
    that no relation mutation exists in the module at all.
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


if __name__ == "__main__":
    unittest.main()
