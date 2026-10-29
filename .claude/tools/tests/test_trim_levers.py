#!/usr/bin/env python3
# cspell:word noseparator
"""Unit tests for trim_levers.py.

Every network call goes through ``_post``, so the probe tests patch that one
seam. ``compose`` touches no API and is tested on embedded parts files.
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

import trim_levers as tl  # noqa: E402
from trim_levers import TrimLeversError, run, validate_fingerprint  # noqa: E402

ENV = {"LINEAR_API_KEY": "key", "LINEAR_PROJECT_ID": "proj"}


def _node(
    identifier="ENG-1",
    title="A batch",
    state="Backlog",
    milestone=None,
    fingerprint="a:b",
    state_type="backlog",
    description=None,
):
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


def _run(argv):
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        try:
            code = run(["trim_levers.py", *argv])
        except TrimLeversError as exc:
            print(f"error: {exc}", file=sys.stderr)
            code = 2
    return code, out.getvalue(), err.getvalue()


class FingerprintTests(unittest.TestCase):
    """The dedup key. If it is wrong, refiling protection is gone."""

    def test_a_well_formed_key_passes(self):
        self.assertEqual(
            validate_fingerprint(" session-metrics:search-scope-axis "),
            "session-metrics:search-scope-axis",
        )

    def test_a_dotted_domain_token_is_refused(self):
        with self.assertRaises(TrimLeversError) as caught:
            validate_fingerprint("http.rs:timeout")
        self.assertIn("linkifies", str(caught.exception))
        self.assertIn("feeds-http", str(caught.exception))

    def test_a_dot_in_the_slug_half_is_allowed(self):
        self.assertEqual(
            validate_fingerprint("claude-tools:review_diff.py-drift"),
            "claude-tools:review_diff.py-drift",
        )

    def test_a_wait_fingerprint_with_a_colon_separated_slug_passes(self):
        self.assertEqual(
            validate_fingerprint("wait:operator:review-pr-merge-queue"),
            "wait:operator:review-pr-merge-queue",
        )

    def test_malformed_keys_are_refused(self):
        for key in (
            "noseparator",
            "   ",
            "Session-Metrics:Foo",
            "wait::cause",
            "wait:op::cause",
            "wait:operator:",
            "wait:Operator:x",
        ):
            with self.subTest(key=key), self.assertRaises(TrimLeversError):
                validate_fingerprint(key)


class ProbeTests(unittest.TestCase):
    def test_the_filter_searches_the_project_bodies_for_the_fingerprint(self):
        seen = []

        def fake(api_key, query, variables):
            seen.append(variables)
            return _page([_node()])

        with mock.patch.object(tl, "_post", side_effect=fake):
            tl.probe("k", "proj", "a:b")
        self.assertEqual(seen[0]["filter"]["description"], {"contains": "a:b"})
        self.assertEqual(seen[0]["filter"]["project"], {"id": {"eq": "proj"}})

    def test_resolved_and_archived_issues_are_searched_too(self):
        # A rejection is a canceled issue, and it must stay permanent.
        self.assertIn("includeArchived: true", tl._PROBE_QUERY)

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

    def test_a_nested_slug_is_not_a_match(self):
        longer = _node("ENG-9", fingerprint="a:search-scope-axis")
        with mock.patch.object(tl, "_post", return_value=_page([longer])):
            self.assertEqual(tl.probe("k", "proj", "a:search-scope"), [])

    def test_a_fingerprint_only_mentioned_in_prose_is_not_a_match(self):
        prose = _node("ENG-9", description="See **Fingerprint**: a:b inline.\n")
        with mock.patch.object(tl, "_post", return_value=_page([prose])):
            self.assertEqual(tl.probe("k", "proj", "a:b"), [])

    def test_a_full_page_with_no_pageinfo_is_refused(self):
        nodes = [_node(f"ENG-{k}") for k in range(tl.PAGE_SIZE)]
        with mock.patch.object(tl, "_post", return_value={"issues": {"nodes": nodes}}):
            with self.assertRaises(TrimLeversError) as caught:
                tl.probe("k", "proj", "a:b")
        self.assertIn("no pageInfo", str(caught.exception))


class ProbeCliTests(unittest.TestCase):
    def test_none_exits_one(self):
        with (
            mock.patch.dict(os.environ, ENV),
            mock.patch.object(tl, "_post", return_value=_page([])),
        ):
            code, out, _ = _run(["probe", "--fingerprint", "a:b"])
        self.assertEqual((code, out.strip()), (1, "NONE a:b"))

    def test_a_rejection_prints_its_state_and_never_the_body(self):
        node = _node(
            "ENG-7",
            state="Canceled",
            state_type="canceled",
            description="SECRET BODY\n\n**Fingerprint**: a:b\n",
        )
        with (
            mock.patch.dict(os.environ, ENV),
            mock.patch.object(tl, "_post", return_value=_page([node])),
        ):
            code, out, _ = _run(["probe", "--fingerprint", "a:b"])
        self.assertEqual(code, 0)
        self.assertIn("MATCH ENG-7 [Canceled:canceled] [-]", out)
        self.assertNotIn("SECRET BODY", out)

    def test_a_dotted_domain_is_refused_before_any_request(self):
        with mock.patch.dict(os.environ, ENV), mock.patch.object(tl, "_post") as post:
            code, _, err = _run(["probe", "--fingerprint", "http.rs:x"])
        self.assertEqual(code, 2)
        self.assertIn("linkifies", err)
        post.assert_not_called()

    def test_the_retired_subcommands_are_gone(self):
        for cmd in ("file", "append-evidence", "list"):
            with self.subTest(cmd=cmd), redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    tl._parse_args(["trim_levers.py", cmd])


PARTS = (
    "## a:first | First part\n"
    "\n"
    "# Lever\n"
    "\n"
    "Do the thing.\n"
    "\n"
    "```md\n"
    "# not a heading\n"
    "**Fingerprint**: x:quoted-example\n"
    "```\n"
    "\n"
    "## a:second | Second part\n"
    "\n"
    "Do the other thing.\n"
    "\n"
    "**Fingerprint**: a:second\n"
    "\n"
    "## b:third | Third part\n"
    "\n"
    "Do a third thing.\n"
)

GROUPS = [
    {"title": "Claude: context economy", "parts": ["a:first", "a:second"]},
    {"title": "Claude: filing conventions", "parts": ["b:third"]},
]


class SplitPartsTests(unittest.TestCase):
    def test_the_section_key_is_the_fingerprint(self):
        parts = tl.split_parts(PARTS)
        self.assertEqual([k for k, _, _ in parts], ["a:first", "a:second", "b:third"])
        self.assertEqual(parts[0][1], "First part")

    def test_a_fenced_example_key_is_not_foreign(self):
        # The quoted `**Fingerprint**:` in part 1 sits inside a fence.
        tl.split_parts(PARTS)

    def test_a_body_key_that_disagrees_with_the_section_is_refused(self):
        text = "## a:one | One\n\nProse.\n\n**Fingerprint**: a:other\n"
        with self.assertRaises(TrimLeversError) as caught:
            tl.split_parts(text)
        self.assertIn("a:one", str(caught.exception))

    def test_a_repeated_key_is_refused(self):
        with self.assertRaises(TrimLeversError) as caught:
            tl.split_parts("## a:one | One\n\nx\n\n## a:one | Again\n\ny\n")
        self.assertIn("more than one part", str(caught.exception))

    def test_a_dotted_section_key_is_refused(self):
        with self.assertRaises(TrimLeversError):
            tl.split_parts("## http.rs:x | One\n\nx\n")

    def test_a_wait_section_key_is_accepted(self):
        text = (
            "## wait:operator:review-pr-merge-queue | Wait\n\nx\n\n"
            "**Fingerprint**: wait:operator:review-pr-merge-queue\n"
        )
        parts = tl.split_parts(text)
        self.assertEqual(parts[0][0], "wait:operator:review-pr-merge-queue")

    def test_no_sections_is_refused(self):
        with self.assertRaises(TrimLeversError):
            tl.split_parts("just prose\n")

    def test_a_fenced_section_heading_stays_inside_its_part(self):
        text = "## a:one | One\n\n```md\n## b:two | quoted\n```\n\nProse.\n"
        parts = tl.split_parts(text)
        self.assertEqual([k for k, _, _ in parts], ["a:one"])
        self.assertIn("## b:two | quoted", parts[0][2])

    def test_a_malformed_section_heading_is_refused(self):
        with self.assertRaises(TrimLeversError) as caught:
            tl.split_parts("## a:one | One\n\nx\n\n## a:two - bad heading\n\ny\n")
        self.assertIn("a:two - bad heading", str(caught.exception))

    def test_text_before_the_first_section_is_refused(self):
        with self.assertRaises(TrimLeversError):
            tl.split_parts("stray\n\n## a:one | One\n\nx\n")

    def test_an_unclosed_fence_is_refused(self):
        text = "## a:one | One\n\n```\nunclosed\n\n## a:two | Two\n\ny\n"
        with self.assertRaises(TrimLeversError) as caught:
            tl.split_parts(text)
        self.assertIn("unclosed fence", str(caught.exception))

    def test_an_empty_part_body_is_refused(self):
        with self.assertRaises(TrimLeversError):
            tl.split_parts("## a:one | One\n\n## a:two | Two\n\ny\n")

    def test_a_duplicated_identical_field_line_is_refused(self):
        text = "## a:one | One\n\n**Fingerprint**: a:one\n\n**Fingerprint**: a:one\n"
        with self.assertRaises(TrimLeversError):
            tl.split_parts(text)

    def test_a_fenced_same_key_still_gets_the_real_field(self):
        body, _ = tl.compose_one("## a:one | One\n\n~~~\n**Fingerprint**: a:one\n~~~\n")
        self.assertEqual(tl.field_values(body, "Fingerprint"), ["a:one"])


class ComposeOneTests(unittest.TestCase):
    def test_every_part_carries_its_fingerprint_exactly_once(self):
        body, summary = tl.compose_one(PARTS)
        for key in ("a:first", "a:second", "b:third"):
            self.assertEqual(body.count(f"**Fingerprint**: {key}\n"), 1, key)
        self.assertEqual(summary["folded"], ["a:first", "a:second", "b:third"])

    def test_headings_demote_but_fenced_lines_do_not(self):
        body, _ = tl.compose_one(PARTS)
        self.assertIn("\n## Lever\n", body)
        self.assertIn("\n# not a heading\n", body)

    def test_start_continues_an_existing_batch(self):
        body, summary = tl.compose_one(PARTS, start=4)
        self.assertIn("# Part 4 — First part", body)
        self.assertIn("# Part 6 — Third part", body)
        self.assertEqual(summary["next_part"], 7)

    def test_start_below_one_is_refused(self):
        with self.assertRaises(TrimLeversError):
            tl.compose_one(PARTS, start=0)


class ComposeGroupsTests(unittest.TestCase):
    def _compose(self, groups):
        return tl.compose_groups(PARTS, tl.parse_groups_spec(json.dumps(groups)))

    def test_one_pass_emits_one_body_per_group_numbered_from_one(self):
        tasks, summary = self._compose(GROUPS)
        self.assertEqual(summary["tasks"], 2)
        self.assertEqual(tasks[0]["folded"], ["a:first", "a:second"])
        self.assertIn("# Part 2 — Second part", tasks[0]["body"])
        self.assertIn("# Part 1 — Third part", tasks[1]["body"])
        self.assertIn("**Fingerprint**: b:third", tasks[1]["body"])

    def test_a_part_in_no_group_is_refused(self):
        with self.assertRaises(TrimLeversError) as caught:
            self._compose([{"title": "Some", "parts": ["a:first"]}])
        self.assertIn("in no group", str(caught.exception))
        self.assertIn("b:third", str(caught.exception))

    def test_a_part_in_two_groups_is_refused(self):
        groups = [
            {"title": "One", "parts": ["a:first", "a:second"]},
            {"title": "Two", "parts": ["a:second", "b:third"]},
        ]
        with self.assertRaises(TrimLeversError) as caught:
            self._compose(groups)
        self.assertIn("more than one group", str(caught.exception))

    def test_a_part_twice_in_one_group_is_named_as_such(self):
        groups = [
            {"title": "One", "parts": ["a:first", "a:first", "a:second", "b:third"]}
        ]
        with self.assertRaises(TrimLeversError) as caught:
            self._compose(groups)
        self.assertIn("twice in group 1", str(caught.exception))

    def test_an_unknown_grouped_part_is_refused(self):
        groups = [{"title": "All", "parts": ["a:first", "a:second", "b:third", "z:x"]}]
        with self.assertRaises(TrimLeversError) as caught:
            self._compose(groups)
        self.assertIn("z:x", str(caught.exception))

    def test_an_oversized_group_is_advisory(self):
        text = "".join(f"## a:p{i} | P{i}\n\nx\n\n" for i in range(6))
        groups = [{"title": "Big", "parts": [f"a:p{i}" for i in range(6)]}]
        _, summary = tl.compose_groups(text, tl.parse_groups_spec(json.dumps(groups)))
        self.assertEqual(summary["oversized"], [("Big", 6)])

    def test_a_title_that_forges_a_summary_line_is_refused(self):
        for title in ("a | b", "a\nb"):
            with self.subTest(title=title), self.assertRaises(TrimLeversError):
                tl.parse_groups_spec(json.dumps([{"title": title, "parts": ["a:x"]}]))

    def test_the_retired_levers_key_is_refused(self):
        with self.assertRaises(TrimLeversError):
            tl.parse_groups_spec(json.dumps([{"title": "T", "levers": ["a:x"]}]))


class ComposeCliTests(unittest.TestCase):
    def test_batch_mode_writes_private_files_and_names_them(self):
        with tempfile.TemporaryDirectory() as tmp:
            parts = os.path.join(tmp, "parts.md")
            groups = os.path.join(tmp, "groups.json")
            out_dir = os.path.join(tmp, "tasks")
            with open(parts, "w", encoding="utf-8") as fh:
                fh.write(PARTS)
            with open(groups, "w", encoding="utf-8") as fh:
                json.dump(GROUPS, fh)
            os.makedirs(out_dir)
            with open(os.path.join(out_dir, "09-stale.md"), "w") as fh:
                fh.write("old")
            # compose needs no credentials at all.
            with mock.patch.dict(os.environ, {}, clear=True):
                code, out, _ = _run(
                    [
                        "compose",
                        "--parts-file",
                        parts,
                        "--groups-file",
                        groups,
                        "--out-dir",
                        out_dir,
                    ]
                )
            self.assertEqual(code, 0)
            first = os.path.join(out_dir, "01-claude-context-economy.md")
            self.assertIn(f"{first} | 2 part(s) | a:first, a:second |", out)
            self.assertEqual(os.stat(first).st_mode & 0o777, 0o600)
            self.assertIn("ADVISORY", out)
            self.assertIn("09-stale.md", out)
            # The body is the payload; it stays on disk.
            self.assertNotIn("Do the thing", out)

    def test_single_mode_prints_the_part_range(self):
        with tempfile.TemporaryDirectory() as tmp:
            parts = os.path.join(tmp, "parts.md")
            dest = os.path.join(tmp, "fold.md")
            with open(parts, "w", encoding="utf-8") as fh:
                fh.write(PARTS)
            code, out, _ = _run(
                ["compose", "--parts-file", parts, "--out", dest, "--start", "3"]
            )
            self.assertEqual(code, 0)
            self.assertIn("parts 3..5", out)
            with open(dest, encoding="utf-8") as fh:
                self.assertIn("# Part 3 — First part", fh.read())

    def test_single_mode_creates_a_missing_out_directory(self):
        with tempfile.TemporaryDirectory() as tmp:
            parts = os.path.join(tmp, "parts.md")
            dest = os.path.join(tmp, "fresh", "fold.md")
            with open(parts, "w", encoding="utf-8") as fh:
                fh.write(PARTS)
            code, _, _ = _run(["compose", "--parts-file", parts, "--out", dest])
            self.assertEqual(code, 0)
            self.assertEqual(os.stat(dest).st_mode & 0o777, 0o600)

    def test_mixed_modes_are_refused(self):
        for argv in (
            ["compose", "--parts-file", "p"],
            ["compose", "--parts-file", "p", "--out", "o", "--groups-file", "g"],
            ["compose", "--parts-file", "p", "--groups-file", "g"],
            ["compose", "--parts-file", "p", "--out", "o", "--out-dir", "d"],
            [
                "compose",
                "--parts-file",
                "p",
                "--groups-file",
                "g",
                "--out-dir",
                "d",
                "--start",
                "2",
            ],
        ):
            with self.subTest(argv=argv), redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    tl._parse_args(["trim_levers.py", *argv])


if __name__ == "__main__":
    unittest.main()
