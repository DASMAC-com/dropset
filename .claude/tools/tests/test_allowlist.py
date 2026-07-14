"""Stdlib ``unittest`` tests for the settings.local.json allowlist helper.

Run via the repo's ``make tools-tests`` (discovery adds ``.claude/tools`` as
the top-level dir so the bare ``import allowlist`` below resolves).
"""

import json
import os
import tempfile
import unittest

from allowlist import AllowlistError, classify, covers, cruft, load_allow


def _settings(allow):
    return {"permissions": {"allow": allow}, "additionalDirectories": ["/some/dir"]}


class LoadTests(unittest.TestCase):
    def test_load_allow_reads_array(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "settings.local.json")
            with open(p, "w", encoding="utf-8") as fh:
                json.dump(_settings(["Bash(git status:*)", "Read(/a/**)"]), fh)
            from pathlib import Path

            self.assertEqual(load_allow(Path(p)), ["Bash(git status:*)", "Read(/a/**)"])

    def test_missing_file_errors(self):
        from pathlib import Path

        with self.assertRaises(AllowlistError):
            load_allow(Path("/no/such/settings.json"))

    def test_malformed_allow_is_empty(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "settings.local.json")
            with open(p, "w", encoding="utf-8") as fh:
                json.dump({"permissions": {}}, fh)
            from pathlib import Path

            self.assertEqual(load_allow(Path(p)), [])


class CoversTests(unittest.TestCase):
    def test_exact_and_subsumed_coverage(self):
        allow = ["Bash(git:*)", "Bash(make lint:*)"]
        # subsumed by the broader git rule
        out = covers("Bash(git status:*)", allow)
        self.assertTrue(out["covered"])
        self.assertEqual(out["insertion_index"], 2)

    def test_uncovered_reports_insertion_and_subsumes(self):
        allow = ["Bash(cargo build:*)", "Bash(cargo test:*)"]
        # a broader cargo rule is not itself covered, and would subsume both
        out = covers("Bash(cargo:*)", allow)
        # is_bareverb_wildcard is a firm-side safety concern, not covers()'s —
        # covers() just reports coverage + subsumption
        self.assertFalse(out["covered"])
        self.assertEqual(out["insertion_index"], 2)
        self.assertEqual(out["would_subsume"], [0, 1])


class ClassifyTests(unittest.TestCase):
    def test_bare_verb_wildcard_is_over_broad(self):
        self.assertEqual(classify("Bash(git:*)", [])[0], "over-broad")

    def test_bare_bash_wildcard_is_over_broad(self):
        self.assertEqual(classify("Bash(:*)", [])[0], "over-broad")

    def test_unscoped_file_root_is_over_broad(self):
        self.assertEqual(classify("Read(/**)", [])[0], "over-broad")
        self.assertEqual(classify("Edit(**)", [])[0], "over-broad")

    def test_dangerous_shapes(self):
        self.assertEqual(classify("Bash(rm -rf build:*)", [])[0], "dangerous")
        self.assertEqual(classify("Bash(git push --force:*)", [])[0], "dangerous")
        # --force-with-lease is the safe form and is NOT flagged
        self.assertIsNone(classify("Bash(git push --force-with-lease:*)", []))

    def test_machine_path(self):
        got = classify("Read(/Users/someone/secrets/**)", [])
        self.assertEqual(got[0], "machine-path")

    def test_subsumed_by_earlier(self):
        # a narrow rule after a broader one is dead weight
        got = classify("Bash(git status:*)", ["Bash(git:*)"])
        self.assertEqual(got[0], "subsumed")

    def test_clean_rule_is_none(self):
        self.assertIsNone(classify("Bash(make lint:*)", ["Bash(git status:*)"]))


class CruftTests(unittest.TestCase):
    def test_flags_only_suspicious_and_keeps_count(self):
        allow = [
            "Bash(git status:*)",  # clean
            "Bash(git:*)",  # over-broad (bare verb)
            "Bash(git status --short:*)",  # subsumed by the bare git verb
            "Read(/Users/me/x/**)",  # machine path
            "Bash(make lint:*)",  # clean
        ]
        out = cruft(allow)
        self.assertEqual(out["count"], 5)
        cats = {f["index"]: f["category"] for f in out["flagged"]}
        self.assertEqual(cats[1], "over-broad")
        self.assertEqual(cats[2], "subsumed")
        self.assertEqual(cats[3], "machine-path")
        # the two clean entries are not in the shortlist
        self.assertNotIn(0, cats)
        self.assertNotIn(4, cats)


if __name__ == "__main__":
    unittest.main()
