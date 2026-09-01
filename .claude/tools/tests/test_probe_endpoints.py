#!/usr/bin/env python3
"""Unit tests for ``probe_endpoints.py`` (stdlib ``unittest``; no pytest).

No network: the pure halves — label parsing and row formatting — carry the
behavior worth asserting, and the redirect flag is the reason the tool exists,
so it is checked directly rather than inferred from a live fetch.
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import probe_endpoints as pe


class ParseLabelUrl(unittest.TestCase):
    def test_a_well_formed_pair_splits(self):
        self.assertEqual(
            pe.parse_label_url("ecb=https://example.invalid/x.csv"),
            ("ecb", "https://example.invalid/x.csv"),
        )

    def test_a_url_containing_equals_keeps_its_query(self):
        # partition, not split: a query string with = must survive intact.
        label, url = pe.parse_label_url("av=https://example.invalid/q?a=1&b=2")
        self.assertEqual(label, "av")
        self.assertEqual(url, "https://example.invalid/q?a=1&b=2")

    def test_a_missing_label_is_refused(self):
        with self.assertRaises(pe.ProbeError):
            pe.parse_label_url("https://example.invalid/x")

    def test_a_label_that_cannot_be_a_filename_is_refused(self):
        # The label names a file in --out-dir, so a slash would escape it.
        with self.assertRaises(pe.ProbeError) as caught:
            pe.parse_label_url("a/b=https://example.invalid/x")
        self.assertIn("filename", str(caught.exception))

    def test_a_non_http_scheme_is_refused(self):
        with self.assertRaises(pe.ProbeError):
            pe.parse_label_url("f=file:///etc/passwd")


class FormatRows(unittest.TestCase):
    @staticmethod
    def row(**over):
        base = {
            "label": "ecb",
            "url": "https://example.invalid/x",
            "status": 200,
            "redirects": 0,
            "bytes": 1234,
            "content_type": "text/csv",
            "final_url": "https://example.invalid/x",
            "error": "",
        }
        base.update(over)
        return base

    def test_a_clean_row_carries_status_size_and_type(self):
        (line,) = pe.format_rows([self.row()])
        self.assertIn("200", line)
        self.assertIn("1234B", line)
        self.assertIn("text/csv", line)
        self.assertNotIn("REDIRECTED", line)

    def test_a_redirect_is_flagged_inline_not_left_to_a_count_column(self):
        # The load-bearing assertion. The feeds client refuses redirects, so
        # "reachable" does not imply "reachable by our client" — and the one
        # session that caught this did so only because its hand-written format
        # string happened to include the count.
        (line,) = pe.format_rows([self.row(redirects=2, final_url="https://other/")])
        self.assertIn("REDIRECTED(2)", line)

    def test_an_http_error_is_a_result_row_not_an_error_row(self):
        # A 451 is exactly the fact being asked for, so it gets a normal row.
        (line,) = pe.format_rows([self.row(status=451)])
        self.assertIn("451", line)
        self.assertNotIn("ERROR", line)

    def test_a_transport_failure_is_an_error_row(self):
        (line,) = pe.format_rows([self.row(error="name resolution failed")])
        self.assertIn("ERROR", line)
        self.assertIn("name resolution failed", line)

    def test_a_missing_content_type_prints_a_dash(self):
        (line,) = pe.format_rows([self.row(content_type="")])
        self.assertIn(" - ", line)


class RunGuards(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.out = str(Path(self._tmp.name) / "bodies")

    def test_no_endpoints_is_refused(self):
        with self.assertRaises(pe.ProbeError):
            pe.run(["probe_endpoints.py", "--out-dir", self.out])

    def test_duplicate_labels_are_refused_before_any_fetch(self):
        # They would silently overwrite one another's body file.
        with self.assertRaises(pe.ProbeError) as caught:
            pe.run(
                [
                    "probe_endpoints.py",
                    "--out-dir",
                    self.out,
                    "--url",
                    "a=https://example.invalid/1",
                    "--url",
                    "a=https://example.invalid/2",
                ]
            )
        self.assertIn("unique", str(caught.exception))

    def test_a_url_file_skips_blanks_and_comments(self):
        path = Path(self._tmp.name) / "urls.txt"
        path.write_text(
            "# a comment\n\na=https://example.invalid/1\n", encoding="utf-8"
        )
        self.assertEqual(pe._read_url_file(str(path)), ["a=https://example.invalid/1"])


if __name__ == "__main__":
    unittest.main()
