#!/usr/bin/env python3
"""Unit tests for vector_diff.py.

The property under test is the one bit the tool exists to answer: a
regeneration that moved only embedded account bytes reads ``bytes-only`` and
exits 0, while any moved expected output reads ``fields``, names the path, and
exits 1. The pure comparison is tested directly; one real throwaway git repo
covers the ref-versus-working-tree read and the exit codes.
"""

from __future__ import annotations

import copy
import io
import json
import os
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import vector_diff as vd

# A trimmed simulate_swap-shaped document: an account buffer, an extra fixture
# buffer keyed by name, and named cases with expected outputs.
BASE = {
    "_comment": "Generated. Do not edit by hand.",
    "market_data": list(range(64)),
    "markets": {"far_out": [7] * 40},
    "cases": [
        {"name": "buy_small", "side": 0, "quote": {"amount_out": 100, "fills": 1}},
        {"name": "sell_big", "side": 1, "quote": {"amount_out": 900, "fills": 3}},
    ],
}


def variant(edit):
    doc = copy.deepcopy(BASE)
    edit(doc)
    return doc


class SummarizeTests(unittest.TestCase):
    def test_identical_documents_are_unchanged(self):
        self.assertEqual(
            vd.summarize(BASE, copy.deepcopy(BASE), 8), ("unchanged", False)
        )

    def test_moved_account_bytes_alone_are_bytes_only(self):
        def widen(doc):
            doc["market_data"] = list(range(72))
            doc["markets"]["far_out"][0] = 8

        line, changed = vd.summarize(BASE, variant(widen), 8)
        self.assertFalse(changed)
        self.assertEqual(
            line, "bytes-only: 2 byte array(s) moved (market_data, markets.far_out)"
        )

    def test_a_moved_expected_output_is_a_field_change(self):
        def bump(doc):
            doc["cases"][1]["quote"]["amount_out"] = 901
            doc["market_data"][0] = 255

        line, changed = vd.summarize(BASE, variant(bump), 8)
        self.assertTrue(changed)
        self.assertIn("cases.sell_big.quote.amount_out: 900 -> 901", line)
        self.assertIn("[1 byte array(s) moved]", line)

    def test_an_inserted_case_is_one_addition_not_a_shifted_index(self):
        def insert(doc):
            doc["cases"].insert(0, {"name": "new_case", "side": 0})

        line, changed = vd.summarize(BASE, variant(insert), 8)
        self.assertTrue(changed)
        self.assertEqual(line, "fields: 1 changed (cases.new_case (added))")

    def test_a_short_integer_list_is_an_output_not_bytes(self):
        old = {"sizes": [1, 2, 3]}
        line, changed = vd.summarize(old, {"sizes": [1, 2, 4]}, 8)
        self.assertTrue(changed)
        self.assertIn("sizes[2]: 3 -> 4", line)

    def test_comment_prose_is_never_a_field(self):
        def reword(doc):
            doc["_comment"] = "Reworded."

        self.assertEqual(
            vd.summarize(BASE, variant(reword), 8),
            ("unchanged [_comment changed]", False),
        )

    def test_the_limit_caps_the_named_paths_and_all_lifts_it(self):
        old = {"v": {str(i): i for i in range(5)}}
        new = {"v": {str(i): i + 1 for i in range(5)}}
        capped, _ = vd.summarize(old, new, 2)
        self.assertIn("fields: 5 changed", capped)
        self.assertIn("+3 more, --all to list", capped)
        full, _ = vd.summarize(old, new, None)
        self.assertNotIn("more", full)
        self.assertIn("v.4: 4 -> 5", full)


class RunTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.repo = Path(cls.tmp.name)
        cls.cwd = os.getcwd()

        def git(*args):
            subprocess.run(
                ["git", *args], cwd=cls.repo, check=True, capture_output=True
            )

        git("init", "-q")
        git("config", "user.email", "t@example.com")
        git("config", "user.name", "T")
        git("config", "commit.gpgsign", "false")
        cls.dir = cls.repo / "sdk" / "conformance"
        cls.dir.mkdir(parents=True)
        for name in ("a.json", "b.json"):
            (cls.dir / name).write_text(json.dumps(BASE, indent=2), encoding="utf-8")
        git("add", "-A")
        git("commit", "-q", "-m", "first")
        os.chdir(cls.repo)

    @classmethod
    def tearDownClass(cls):
        os.chdir(cls.cwd)
        cls.tmp.cleanup()

    def _write(self, name, doc):
        (self.dir / name).write_text(json.dumps(doc, indent=2), encoding="utf-8")

    def _run(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = vd.run(["vector_diff.py"] + argv)
        return code, out.getvalue(), err.getvalue()

    def tearDown(self):
        for name in ("a.json", "b.json"):
            self._write(name, BASE)

    def test_bytes_only_regeneration_exits_zero_across_the_default_glob(self):
        def widen(doc):
            doc["market_data"] = list(range(80))

        self._write("a.json", variant(widen))
        code, out, _ = self._run([])
        self.assertEqual(code, 0)
        self.assertEqual(
            out.splitlines(),
            [
                "a.json: bytes-only: 1 byte array(s) moved (market_data)",
                "b.json: unchanged",
            ],
        )

    def test_a_field_change_exits_one(self):
        def bump(doc):
            doc["cases"][0]["quote"]["fills"] = 2

        self._write("b.json", variant(bump))
        code, out, _ = self._run(["sdk/conformance/b.json"])
        self.assertEqual(code, 1)
        self.assertIn("cases.buy_small.quote.fills: 1 -> 2", out)

    def test_a_path_missing_at_the_ref_is_an_error(self):
        self._write("c.json", BASE)
        try:
            with self.assertRaises(vd.VectorDiffError):
                self._run(["sdk/conformance/c.json"])
        finally:
            (self.dir / "c.json").unlink()


if __name__ == "__main__":
    unittest.main()
