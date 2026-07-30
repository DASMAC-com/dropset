"""Stdlib ``unittest`` tests for the settings.local.json allowlist helper.

Run via the repo's ``make tools-tests`` (discovery adds ``.claude/tools`` as
the top-level dir so the bare ``import allowlist`` below resolves).
"""

import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

from allowlist import (
    AllowlistError,
    add,
    classify,
    covers,
    cruft,
    load_allow,
    run,
)


def _settings(allow):
    return {"permissions": {"allow": allow}, "additionalDirectories": ["/some/dir"]}


class LoadTests(unittest.TestCase):
    def test_load_allow_reads_array(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "settings.local.json")
            with open(p, "w", encoding="utf-8") as fh:
                json.dump(_settings(["Bash(git status:*)", "Read(/a/**)"]), fh)
            self.assertEqual(load_allow(Path(p)), ["Bash(git status:*)", "Read(/a/**)"])

    def test_missing_file_errors(self):
        with self.assertRaises(AllowlistError):
            load_allow(Path("/no/such/settings.json"))

    def test_malformed_allow_is_empty(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "settings.local.json")
            with open(p, "w", encoding="utf-8") as fh:
                json.dump({"permissions": {}}, fh)
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
    def _solo(self, rule):
        # classify a rule as the only entry (index 0) — no subsumption context
        return classify(rule, 0, [rule])

    def test_bare_verb_wildcard_is_over_broad(self):
        self.assertEqual(self._solo("Bash(git:*)")[0], "over-broad")

    def test_bare_bash_wildcard_is_over_broad(self):
        self.assertEqual(self._solo("Bash(:*)")[0], "over-broad")

    def test_unscoped_file_root_is_over_broad(self):
        self.assertEqual(self._solo("Read(/**)")[0], "over-broad")
        self.assertEqual(self._solo("Edit(**)")[0], "over-broad")

    def test_dangerous_shapes(self):
        self.assertEqual(self._solo("Bash(rm -rf build:*)")[0], "dangerous")
        self.assertEqual(self._solo("Bash(git push --force:*)")[0], "dangerous")
        # --force-with-lease is the safe form and is NOT flagged
        self.assertIsNone(self._solo("Bash(git push --force-with-lease:*)"))

    def test_machine_path(self):
        self.assertEqual(
            self._solo("Read(/Users/someone/secrets/**)")[0], "machine-path"
        )

    def test_subsumed_broad_before_narrow(self):
        allow = ["Bash(git status:*)", "Bash(git status --short:*)"]
        self.assertEqual(classify(allow[1], 1, allow)[0], "subsumed")
        self.assertIsNone(classify(allow[0], 0, allow))  # the broad one stays

    def test_subsumed_narrow_before_broad_append_pattern(self):
        # firm-perms appends the broader rule AFTER the narrow one — the narrow
        # entry is still dead weight and must be flagged regardless of order.
        allow = ["Bash(git status --short:*)", "Bash(git status:*)"]
        self.assertEqual(classify(allow[0], 0, allow)[0], "subsumed")
        self.assertIsNone(classify(allow[1], 1, allow))

    def test_over_broad_coverer_does_not_subsume(self):
        # covered only by an over-broad bare-verb rule (itself flagged for
        # removal) → not reported as subsumed dead weight.
        allow = ["Bash(git status:*)", "Bash(git:*)"]
        self.assertIsNone(classify(allow[0], 0, allow))
        self.assertEqual(classify(allow[1], 1, allow)[0], "over-broad")

    def test_exact_duplicate_flags_only_the_later_copy(self):
        allow = ["Bash(make lint:*)", "Bash(make lint:*)"]
        self.assertIsNone(classify(allow[0], 0, allow))
        self.assertEqual(classify(allow[1], 1, allow)[0], "subsumed")

    def test_clean_rule_is_none(self):
        allow = ["Bash(git status:*)", "Bash(make lint:*)"]
        self.assertIsNone(classify(allow[1], 1, allow))


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


class AddTests(unittest.TestCase):
    """``add`` is the write counterpart of ``covers`` — no prior read required."""

    def _path(self, d, allow):
        p = Path(d) / "settings.local.json"
        p.write_text(json.dumps(_settings(allow)), encoding="utf-8")
        return p

    def test_appends_an_uncovered_rule(self):
        with tempfile.TemporaryDirectory() as d:
            p = self._path(d, ["Bash(git status:*)"])
            out = add("Bash(cargo test:*)", p)
            self.assertTrue(out["added"])
            self.assertFalse(out["covered"])
            self.assertEqual(out["count"], 2)
            self.assertEqual(
                load_allow(p), ["Bash(git status:*)", "Bash(cargo test:*)"]
            )

    def test_is_idempotent_for_an_exact_duplicate(self):
        with tempfile.TemporaryDirectory() as d:
            p = self._path(d, ["Bash(cargo test:*)"])
            out = add("Bash(cargo test:*)", p)
            self.assertFalse(out["added"])
            self.assertTrue(out["covered"])
            self.assertEqual(load_allow(p), ["Bash(cargo test:*)"])

    def test_skips_a_rule_a_broader_entry_already_covers(self):
        with tempfile.TemporaryDirectory() as d:
            p = self._path(d, ["Bash(git:*)"])
            out = add("Bash(git status:*)", p)
            self.assertFalse(out["added"])
            self.assertEqual(load_allow(p), ["Bash(git:*)"])

    def test_prunes_entries_the_new_rule_subsumes(self):
        with tempfile.TemporaryDirectory() as d:
            p = self._path(
                d, ["Bash(cargo test -p a:*)", "Bash(cargo test -p b:*)", "Read(/x/**)"]
            )
            out = add("Bash(cargo test:*)", p)
            self.assertTrue(out["added"])
            self.assertEqual(load_allow(p), ["Read(/x/**)", "Bash(cargo test:*)"])
            self.assertEqual(out["count"], 2)

    def test_preserves_unrelated_settings_keys(self):
        with tempfile.TemporaryDirectory() as d:
            p = self._path(d, ["Bash(git status:*)"])
            add("Bash(cargo test:*)", p)
            settings = json.loads(p.read_text(encoding="utf-8"))
            self.assertEqual(settings["additionalDirectories"], ["/some/dir"])

    def test_scaffolds_a_missing_settings_file(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "nested" / "settings.local.json"
            out = add("Bash(cargo test:*)", p)
            self.assertTrue(out["added"])
            self.assertEqual(load_allow(p), ["Bash(cargo test:*)"])


class CliTests(unittest.TestCase):
    """The ``--settings`` option + subcommand dispatch live in ``run()``."""

    def _write(self, d, allow):
        p = os.path.join(d, "settings.local.json")
        with open(p, "w", encoding="utf-8") as fh:
            json.dump(_settings(allow), fh)
        return p

    def _run_capture(self, argv):
        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = run(argv)
        return rc, json.loads(buf.getvalue())

    def test_covers_dispatch(self):
        with tempfile.TemporaryDirectory() as d:
            p = self._write(d, ["Bash(git:*)"])
            rc, out = self._run_capture(
                ["allowlist.py", "--settings", p, "covers", "Bash(git status:*)"]
            )
        self.assertEqual(rc, 0)
        self.assertTrue(out["covered"])

    def test_cruft_dispatch(self):
        with tempfile.TemporaryDirectory() as d:
            p = self._write(d, ["Bash(git:*)", "Read(/Users/me/x/**)"])
            rc, out = self._run_capture(["allowlist.py", "--settings", p, "cruft"])
        self.assertEqual(rc, 0)
        self.assertEqual(out["count"], 2)
        self.assertEqual(
            {f["category"] for f in out["flagged"]}, {"over-broad", "machine-path"}
        )

    def test_add_dispatch_writes_the_rule(self):
        with tempfile.TemporaryDirectory() as d:
            p = self._write(d, ["Bash(git status:*)"])
            rc, out = self._run_capture(
                ["allowlist.py", "--settings", p, "add", "Bash(cargo test:*)"]
            )
            self.assertEqual(rc, 0)
            self.assertTrue(out["added"])
            self.assertIn("Bash(cargo test:*)", load_allow(Path(p)))

    def test_add_dispatch_on_a_missing_file_does_not_error(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "settings.local.json")
            rc, out = self._run_capture(
                ["allowlist.py", "--settings", p, "add", "Bash(cargo test:*)"]
            )
            self.assertEqual(rc, 0)
            self.assertTrue(out["added"])


if __name__ == "__main__":
    unittest.main()
