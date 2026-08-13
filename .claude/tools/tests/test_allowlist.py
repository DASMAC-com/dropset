# cspell:word unparseable
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

import firm_core

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

    def test_a_machine_local_file_does_not_flag_absolute_paths(self):
        """The false-positive fix: `settings.local.json` is git-ignored and
        machine-local by design, so an absolute home path is the correct form
        there. Flagging it made 39 of 40 shortlist entries noise, nearly all of
        them load-bearing worktree and skill-tooling rules.

        The path has to be a real one — an absolute path that resolves is the
        clean case; one that doesn't is `machine-path-stale`, tested below.
        """
        home = Path.home()
        rule = f"Bash(git -C {home}/* status:*)"
        self.assertIsNone(classify(rule, 0, [rule], machine_local=True))

    def test_a_malformed_path_is_flagged_even_when_machine_local(self):
        """The one true positive that pass found: a doubled leading slash means
        the rule can never match, in any settings file."""
        rule = "Read(//Users/me/.cargo/**)"
        verdict = classify(rule, 0, [rule], machine_local=True)
        self.assertEqual(verdict[0], "machine-path")
        self.assertIn("doubled slash", verdict[1])

    def test_a_doubled_slash_mid_path_is_also_malformed(self):
        """Not just a doubled *leading* slash — `/a/b//c` can never match
        either, and the first lookbehind was narrow enough to miss it."""
        rule = "Read(/Users/me/repos//dropset/**)"
        verdict = classify(rule, 0, [rule], machine_local=True)
        self.assertEqual(verdict[0], "machine-path")
        self.assertIn("doubled slash", verdict[1])

    def test_a_url_scheme_is_not_read_as_a_doubled_slash(self):
        """`https://` must stay clean — it is the reason the check needs a
        lookbehind at all."""
        rule = "Bash(curl https://example.com/x:*)"
        self.assertIsNone(classify(rule, 0, [rule], machine_local=True))

    def test_a_trailing_arg_separator_is_not_part_of_the_path(self):
        """`Bash(python3 /abs/tool.py:*)` must resolve `/abs/tool.py`, not
        `/abs/tool.py:` — otherwise every absolute Bash rule reads as stale."""
        real = Path(__file__).resolve()
        rule = f"Bash(python3 {real}:*)"
        self.assertIsNone(classify(rule, 0, [rule], machine_local=True))

    def test_a_stale_path_is_flagged_when_machine_local(self):
        """Where absolute paths are legitimate, a path that no longer resolves
        is the check with real value — worktree rules decay this way."""
        rule = "Bash(python3 /Users/nobody/definitely-not-here/tool.py:*)"
        verdict = classify(rule, 0, [rule], machine_local=True)
        self.assertEqual(verdict[0], "machine-path-stale")
        self.assertIn("no longer exists", verdict[1])

    def test_a_resolving_path_is_clean_when_machine_local(self):
        rule = f"Read({Path(__file__).parent}/**)"
        self.assertIsNone(classify(rule, 0, [rule], machine_local=True))

    def test_a_shared_file_reports_the_absolute_path_not_its_staleness(self):
        """In a shared settings file the absolute path is itself the defect, so
        whether it resolves is beside the point."""
        rule = "Bash(python3 /Users/nobody/definitely-not-here/tool.py:*)"
        self.assertEqual(
            classify(rule, 0, [rule], machine_local=False)[0], "machine-path"
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


class CruftFileAwarenessTests(unittest.TestCase):
    def test_a_local_settings_path_suppresses_absolute_path_flags(self):
        allow = [
            "Bash(git -C /Users/me/repo/.claude/worktrees/*/ status:*)",
            "Read(/Users/me/.zshrc)",
        ]
        out = cruft(allow, Path("/Users/me/repo/.claude/settings.local.json"))
        self.assertTrue(out["machine_local_settings"])
        self.assertEqual(
            [f["category"] for f in out["flagged"]], ["machine-path-stale"] * 2
        )

    def test_a_shared_settings_path_still_flags_them(self):
        allow = ["Read(/Users/me/.zshrc)"]
        out = cruft(allow, Path("/repo/.claude/settings.json"))
        self.assertFalse(out["machine_local_settings"])
        self.assertEqual(out["flagged"][0]["category"], "machine-path")

    def test_no_settings_path_defaults_to_the_strict_reading(self):
        out = cruft(["Read(/Users/me/.zshrc)"])
        self.assertFalse(out["machine_local_settings"])
        self.assertEqual(out["flagged"][0]["category"], "machine-path")


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
        self.assertFalse(out["machine_local_settings"])
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

    def test_refuses_a_bare_verb_wildcard_that_slash_f_would_refuse(self):
        """`firm_into` has no floor of its own — `firm_last` checks it in the
        caller — so a write path that skipped the check would grant exactly what
        /f refuses, via one non-prompting pre-approved call."""
        for rule in ("Bash(git:*)", "Bash(rm:*)", "Bash(curl:*)"):
            with tempfile.TemporaryDirectory() as d:
                p = self._path(d, ["Bash(git status:*)"])
                out = add(rule, p)
                self.assertFalse(out["added"], rule)
                self.assertIsNotNone(out["refused"], rule)
                # The file is untouched.
                self.assertEqual(load_allow(p), ["Bash(git status:*)"])

    def test_refuses_a_bare_wildcard_and_an_unscoped_file_root(self):
        for rule in ("Bash(:*)", "Bash(*)", "Read(/**)", "Edit(**)"):
            with tempfile.TemporaryDirectory() as d:
                p = self._path(d, [])
                out = add(rule, p)
                self.assertFalse(out["added"], rule)
                self.assertEqual(load_allow(p), [], rule)

    def test_refusal_never_writes_even_a_scaffold(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "settings.local.json"
            out = add("Bash(git:*)", p)
            self.assertFalse(out["added"])
            self.assertFalse(p.exists())

    def test_a_narrow_rule_the_floor_allows_still_writes(self):
        with tempfile.TemporaryDirectory() as d:
            p = self._path(d, [])
            out = add("Bash(git status:*)", p)
            self.assertTrue(out["added"])
            self.assertIsNone(out["refused"])

    def test_written_settings_file_is_owner_only(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "settings.local.json"
            add("Bash(cargo test:*)", p)
            self.assertEqual(p.stat().st_mode & 0o777, 0o600)

    def test_refuses_to_clobber_an_unparseable_settings_file(self):
        """A stray trailing comma, or a mistyped --settings pointing at some
        other JSON, must not be silently replaced by fresh scaffolding."""
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "settings.local.json"
            p.write_text('{"permissions": {"allow": ["a"],}}', encoding="utf-8")
            with self.assertRaises(firm_core.SettingsError):
                add("Bash(cargo test:*)", p)
            # Original bytes survive.
            self.assertIn("allow", p.read_text(encoding="utf-8"))

    def test_leaves_no_temp_file_behind(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "settings.local.json"
            add("Bash(cargo test:*)", p)
            self.assertEqual(
                sorted(x.name for x in Path(d).iterdir()), ["settings.local.json"]
            )


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
        # The settings file is a `settings.local.json`, where an absolute home
        # path is expected — so the actionable signal is that it no longer
        # resolves, not that it is absolute.
        self.assertTrue(out["machine_local_settings"])
        self.assertEqual(
            {f["category"] for f in out["flagged"]},
            {"over-broad", "machine-path-stale"},
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

    def test_add_dispatch_exits_non_zero_when_the_floor_refuses(self):
        with tempfile.TemporaryDirectory() as d:
            p = self._write(d, ["Bash(git status:*)"])
            rc, out = self._run_capture(
                ["allowlist.py", "--settings", p, "add", "Bash(git:*)"]
            )
            self.assertEqual(rc, 1)
            self.assertFalse(out["added"])
            self.assertIsNotNone(out["refused"])


if __name__ == "__main__":
    unittest.main()
