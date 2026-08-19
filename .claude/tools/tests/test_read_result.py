#!/usr/bin/env python3
"""Unit tests for read_result.py.

Everything here works on temporary files: the tool's whole job is to read a
payload in its own process, so the tests give it real files and assert on what
reaches stdout — which is the thing that would otherwise reach a transcript.
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

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import read_result as rr  # noqa: E402
from read_result import ReadResultError, extract_field, headings, run, section, unwrap  # noqa: E402

BODY = "\n".join(
    [
        "Intro prose.",
        "",
        "# Part 1 — first",
        "",
        "**Fingerprint**: a:one",
        "body of one",
        "",
        "## Part 1 detail",
        "",
        "nested under one",
        "",
        "# Part 2 — second",
        "",
        "body of two",
    ]
)


def _persisted(payload):
    """A file in the harness's spilled-tool-result shape."""
    d = tempfile.mkdtemp()
    path = Path(d) / "result.json"
    path.write_text(
        json.dumps([{"type": "text", "text": json.dumps(payload)}]), encoding="utf-8"
    )
    return str(path)


def _plain(text):
    d = tempfile.mkdtemp()
    path = Path(d) / "plain.txt"
    path.write_text(text, encoding="utf-8")
    return str(path)


def _invoke(*argv):
    """Run the CLI, returning ``(rc, stdout, stderr)``."""
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        rc = run(["read_result.py", *argv])
    return rc, out.getvalue(), err.getvalue()


class UnwrapTests(unittest.TestCase):
    def test_the_block_array_envelope_is_unwrapped(self):
        raw = json.dumps([{"type": "text", "text": "hello"}])
        self.assertEqual(unwrap(raw), "hello")

    def test_several_blocks_are_joined(self):
        raw = json.dumps(
            [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]
        )
        self.assertEqual(unwrap(raw), "a\nb")

    def test_a_bare_json_string_is_unwrapped(self):
        self.assertEqual(unwrap(json.dumps("hello")), "hello")

    def test_plain_text_passes_through(self):
        self.assertEqual(unwrap("not json at all"), "not json at all")

    def test_a_truncated_spill_is_still_returned_for_grepping(self):
        # The point: a half-written file must remain searchable rather than
        # raising, because a truncated payload is exactly when you need to look.
        self.assertEqual(unwrap('[{"type": "text", "te'), '[{"type": "text", "te')

    def test_a_block_array_without_text_keys_falls_back_to_raw(self):
        raw = json.dumps([{"type": "image"}])
        self.assertEqual(unwrap(raw), raw)


class ExtractFieldTests(unittest.TestCase):
    def test_a_top_level_field(self):
        self.assertEqual(extract_field(json.dumps({"description": "hi"}), "description"), "hi")

    def test_a_dotted_path_and_a_list_index(self):
        text = json.dumps({"attachments": [{"url": "u0"}, {"url": "u1"}]})
        self.assertEqual(extract_field(text, "attachments.1.url"), "u1")

    def test_a_non_string_value_is_serialized_readably(self):
        got = extract_field(json.dumps({"labels": ["a", "b"]}), "labels")
        self.assertEqual(json.loads(got), ["a", "b"])

    def test_a_missing_key_names_the_alternatives(self):
        text = json.dumps({"title": "t", "description": "d"})
        with self.assertRaises(ReadResultError) as caught:
            extract_field(text, "body")
        message = str(caught.exception)
        self.assertIn("no key 'body'", message)
        self.assertIn("description", message)

    def test_a_bad_list_index_is_a_clean_error(self):
        text = json.dumps({"a": [1]})
        with self.assertRaises(ReadResultError):
            extract_field(text, "a.7")

    def test_descending_into_a_scalar_is_a_clean_error(self):
        with self.assertRaises(ReadResultError):
            extract_field(json.dumps({"a": 1}), "a.b")

    def test_a_non_json_payload_says_so(self):
        with self.assertRaises(ReadResultError) as caught:
            extract_field("plain text", "description")
        self.assertIn("not JSON", str(caught.exception))


class HeadingsTests(unittest.TestCase):
    def test_headings_carry_line_numbers_and_depth_indent(self):
        got = headings(BODY.splitlines())
        self.assertEqual(
            got,
            ["3:Part 1 — first", "8:  Part 1 detail", "12:Part 2 — second"],
        )

    def test_a_hash_inside_prose_is_not_a_heading(self):
        self.assertEqual(headings(["a # b", "#nospace"]), [])


class SectionTests(unittest.TestCase):
    def test_a_section_runs_to_the_next_heading_of_the_same_depth(self):
        block, start = section(BODY.splitlines(), "Part 1")
        self.assertEqual(start, 3)
        # Its own nested subsection comes with it; the next part does not.
        self.assertIn("## Part 1 detail", block)
        self.assertIn("nested under one", block)
        self.assertNotIn("# Part 2 — second", block)

    def test_the_last_section_runs_to_the_end(self):
        block, _ = section(BODY.splitlines(), "Part 2")
        self.assertEqual(block[-1], "body of two")

    def test_matching_is_case_insensitive(self):
        block, _ = section(BODY.splitlines(), "part 2")
        self.assertIn("body of two", block)

    def test_no_match_points_at_the_navigation_mode(self):
        with self.assertRaises(ReadResultError) as caught:
            section(BODY.splitlines(), "Part 40")
        self.assertIn("--headings", str(caught.exception))

    def test_a_bad_regex_is_a_clean_error(self):
        with self.assertRaises(ReadResultError):
            section(BODY.splitlines(), "Part (")


class GrepTests(unittest.TestCase):
    def test_zero_context_is_the_default(self):
        got = rr.grep(BODY.splitlines(), "Fingerprint", 0)
        self.assertEqual(got, ["5:**Fingerprint**: a:one"])

    def test_context_widens_the_window(self):
        got = rr.grep(BODY.splitlines(), "Fingerprint", 1)
        self.assertEqual(len(got), 3)

    def test_separated_runs_are_marked(self):
        got = rr.grep(BODY.splitlines(), "^# Part", 0)
        self.assertEqual(got, ["3:# Part 1 — first", "--", "12:# Part 2 — second"])

    def test_no_match_returns_nothing(self):
        self.assertEqual(rr.grep(BODY.splitlines(), "absent", 2), [])


class SliceTests(unittest.TestCase):
    def test_a_range_is_inclusive_and_one_indexed(self):
        self.assertEqual(rr.parse_slice("3:5", 20), (3, 5))

    def test_open_sides_default_to_the_ends(self):
        self.assertEqual(rr.parse_slice(":4", 20), (1, 4))
        self.assertEqual(rr.parse_slice("18:", 20), (18, 20))

    def test_a_range_past_the_end_is_clamped_not_refused(self):
        self.assertEqual(rr.parse_slice("19:900", 20), (19, 20))

    def test_a_malformed_range_is_a_clean_error(self):
        for spec in ("5", "a:b", "19:2"):
            with self.assertRaises(ReadResultError):
                rr.parse_slice(spec, 20)


class CliTests(unittest.TestCase):
    def test_headings_over_a_persisted_linear_body(self):
        path = _persisted({"description": BODY})
        rc, out, err = _invoke(path, "--field", "description", "--headings")
        self.assertEqual(rc, 0)
        self.assertIn("Part 2 — second", out)
        self.assertNotIn("body of two", out)
        self.assertIn("3 heading(s)", err)

    def test_section_emits_one_part_only(self):
        path = _persisted({"description": BODY})
        rc, out, _ = _invoke(path, "--field", "description", "--section", "Part 2")
        self.assertEqual(rc, 0)
        self.assertIn("body of two", out)
        self.assertNotIn("body of one", out)

    def test_count_reports_size_without_emitting_the_payload(self):
        path = _persisted({"description": BODY})
        rc, out, _ = _invoke(path, "--field", "description", "--count")
        self.assertEqual(rc, 0)
        self.assertIn("heading(s)", out)
        self.assertNotIn("body of one", out)

    def test_diff_reports_only_what_changed(self):
        old = _persisted({"description": BODY})
        new = _persisted({"description": BODY + "\n\nan appended addendum"})
        rc, out, err = _invoke(new, "--field", "description", "--diff", old)
        self.assertEqual(rc, 0)
        self.assertIn("+an appended addendum", out)
        # The unchanged bulk stays out of the output — the whole reason the mode
        # exists is that re-reading an amended body costs as much as the first read.
        self.assertNotIn("Intro prose.", out)
        self.assertIn("diff line(s)", err)

    def test_diff_of_identical_payloads_says_so(self):
        a = _persisted({"description": BODY})
        b = _persisted({"description": BODY})
        rc, out, err = _invoke(b, "--field", "description", "--diff", a)
        self.assertEqual(rc, 0)
        self.assertEqual(out, "")
        self.assertIn("identical", err)

    def test_it_works_on_a_plain_file_with_no_envelope(self):
        rc, out, _ = _invoke(_plain(BODY), "--grep", "Fingerprint")
        self.assertEqual(rc, 0)
        self.assertIn("a:one", out)

    def test_no_mode_is_refused_with_the_list_of_modes(self):
        with self.assertRaises(ReadResultError) as caught:
            _invoke(_plain(BODY))
        self.assertIn("--headings", str(caught.exception))

    def test_a_missing_file_is_a_clean_error(self):
        with self.assertRaises(ReadResultError) as caught:
            _invoke("/nonexistent/nope.json", "--count")
        self.assertIn("cannot read", str(caught.exception))

    def test_main_turns_an_error_into_exit_two(self):
        argv = ["read_result.py", "/nonexistent/nope.json", "--count"]
        with mock_argv(argv):
            with redirect_stderr(io.StringIO()) as err:
                rc = rr.main()
        self.assertEqual(rc, 2)
        self.assertIn("error:", err.getvalue())


class mock_argv:  # noqa: N801 — a tiny context manager, not a class API
    def __init__(self, argv):
        self.argv = argv

    def __enter__(self):
        self.saved = sys.argv
        sys.argv = self.argv

    def __exit__(self, *exc):
        sys.argv = self.saved
        return False


if __name__ == "__main__":
    unittest.main()
