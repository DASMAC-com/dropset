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

from unittest import mock

import allowlist
import firm_core

from allowlist import (
    DEFAULT_SETTINGS,
    AllowlistError,
    add,
    classify,
    covers,
    cruft,
    load_allow,
    resolve_settings_path,
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

    def test_missing_file_is_empty_not_an_error(self):
        """Under the shared-file model a worktree has no copy of its own, so
        an absent file is the normal case rather than a bad path."""
        self.assertEqual(load_allow(Path("/no/such/settings.json")), [])

    def test_malformed_file_still_errors(self):
        """A corrupt file is a real defect — treating it as empty would hide
        it, which is the one thing the permissive missing-file path must not
        start doing."""
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "settings.local.json")
            with open(p, "w", encoding="utf-8") as fh:
                fh.write("{not json")
            with self.assertRaises(AllowlistError):
                load_allow(Path(p))

    def test_malformed_allow_is_empty(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "settings.local.json")
            with open(p, "w", encoding="utf-8") as fh:
                json.dump({"permissions": {}}, fh)
            self.assertEqual(load_allow(Path(p)), [])


class ResolveSettingsPathTests(unittest.TestCase):
    """The contract: the DEFAULT always resolves to the main checkout; an
    EXPLICIT path is never redirected. Note these patch
    `allowlist.firm_core.main_settings_path`, the single owner — patching a
    re-exported alias would leave the real resolver running and silently reach
    the developer's own checkout."""

    def _base(self, d, rules=("Bash(ls:*)",)):
        base = Path(d) / "base"
        (base / ".claude").mkdir(parents=True)
        (base / DEFAULT_SETTINGS).write_text(
            json.dumps(_settings(list(rules))), encoding="utf-8"
        )
        return base

    def test_the_default_resolves_to_the_main_checkout(self):
        with tempfile.TemporaryDirectory() as d:
            base = self._base(d)
            with mock.patch(
                "allowlist.firm_core.main_settings_path",
                return_value=base / DEFAULT_SETTINGS,
            ):
                self.assertEqual(resolve_settings_path(None), base / DEFAULT_SETTINGS)

    def test_the_default_resolves_there_even_when_that_file_does_not_exist_yet(
        self,
    ):
        """The bug this fixes: keying the fallback on `resolved.exists()` sent
        `add` to scaffold a worktree-local file that nothing ever reads, which
        then shadowed the real one forever."""
        with tempfile.TemporaryDirectory() as d:
            absent = Path(d) / "base" / DEFAULT_SETTINGS
            with mock.patch(
                "allowlist.firm_core.main_settings_path", return_value=absent
            ):
                self.assertEqual(resolve_settings_path(None), absent)

    def test_with_no_main_checkout_it_falls_back_to_the_cwd_default(self):
        with mock.patch("allowlist.firm_core.main_settings_path", return_value=None):
            self.assertEqual(resolve_settings_path(None), Path(DEFAULT_SETTINGS))

    def test_an_explicit_path_is_never_redirected(self):
        """A caller that names a file means that file — retargeting it would
        send an `add` write to a different allowlist than the one asked for."""
        with tempfile.TemporaryDirectory() as d:
            base = self._base(d)
            named = Path(d) / "elsewhere" / "settings.local.json"
            with mock.patch(
                "allowlist.firm_core.main_settings_path",
                return_value=base / DEFAULT_SETTINGS,
            ):
                self.assertEqual(resolve_settings_path(named, explicit=True), named)

    def test_an_explicit_path_equal_to_the_default_string_is_still_explicit(self):
        """Value-equality against DEFAULT_SETTINGS could not tell "not passed"
        from "passed the literal default"; run() uses a None sentinel."""
        with tempfile.TemporaryDirectory() as d:
            base = self._base(d)
            named = Path(DEFAULT_SETTINGS)
            with mock.patch(
                "allowlist.firm_core.main_settings_path",
                return_value=base / DEFAULT_SETTINGS,
            ):
                self.assertEqual(resolve_settings_path(named, explicit=True), named)

    def test_resolution_then_load_yields_the_main_checkout_allowlist(self):
        """The end-to-end shape of the bug: a worktree with no settings file
        of its own read an empty allowlist (or errored) instead of the real
        one at the main checkout."""
        with tempfile.TemporaryDirectory() as d:
            base = self._base(d, ["Bash(git status:*)"])
            with mock.patch(
                "allowlist.firm_core.main_settings_path",
                return_value=base / DEFAULT_SETTINGS,
            ):
                allow = load_allow(resolve_settings_path(None))
            self.assertEqual(allow, ["Bash(git status:*)"])


class WorktreeScanTests(unittest.TestCase):
    """`main_checkout`'s parsing half, which was previously only ever mocked."""

    def test_finds_the_worktree_on_main(self):
        porcelain = (
            "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\n"
            "worktree /repo/.claude/worktrees/eng-1\nHEAD def\n"
            "branch refs/heads/eng-1\n"
        )
        self.assertEqual(firm_core.parse_worktree_list(porcelain), Path("/repo"))

    def test_finds_main_when_it_is_not_first(self):
        porcelain = (
            "worktree /repo/wt\nHEAD def\nbranch refs/heads/eng-1\n\n"
            "worktree /repo\nHEAD abc\nbranch refs/heads/main\n"
        )
        self.assertEqual(firm_core.parse_worktree_list(porcelain), Path("/repo"))

    def test_returns_none_when_nothing_is_on_main(self):
        porcelain = "worktree /repo\nHEAD abc\nbranch refs/heads/feature\n"
        self.assertIsNone(firm_core.parse_worktree_list(porcelain))

    def test_a_detached_worktree_does_not_match(self):
        porcelain = "worktree /repo\nHEAD abc\ndetached\n"
        self.assertIsNone(firm_core.parse_worktree_list(porcelain))

    def test_empty_output_returns_none(self):
        self.assertIsNone(firm_core.parse_worktree_list(""))


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


class GuardConflictTests(unittest.TestCase):
    """The semantic pass: a rule granting what a no-escape-hatch guard blocks.

    Every other category judges the rule's own text; this one judges the rule
    against a convention enforced elsewhere, which is why it exists.
    """

    def _solo(self, rule):
        return classify(rule, 0, [rule], machine_local=True)

    def test_the_live_worktree_git_grep_grant_is_flagged(self):
        # The real entry that motivated this, verbatim in shape.
        rule = "Bash(git -C /Users/me/repo/.claude/worktrees/* grep:*)"
        category, reason = self._solo(rule)
        self.assertEqual(category, "guard-conflict")
        self.assertIn("git grep", reason)

    def test_a_plain_git_grep_grant_is_flagged(self):
        self.assertEqual(self._solo("Bash(git grep:*)")[0], "guard-conflict")

    def test_the_guards_own_option_forms_are_covered(self):
        # Delegating to the guard's predicate is what buys these for free —
        # re-deriving "what counts as git grep" here would have missed them.
        for rule in (
            "Bash(git --no-pager grep:*)",
            "Bash(git -c core.pager=cat grep:*)",
            "Bash(git --git-dir=/x/.git grep:*)",
        ):
            with self.subTest(rule=rule):
                self.assertEqual(self._solo(rule)[0], "guard-conflict")

    def test_an_ordinary_git_rule_is_not_flagged(self):
        for rule in (
            "Bash(git status:*)",
            "Bash(git log --grep=foo:*)",
            "Bash(git -C /Users/me/repo/.claude/worktrees/* status:*)",
        ):
            with self.subTest(rule=rule):
                self.assertIsNone(allowlist.guard_conflict(rule))

    def test_a_bare_grep_grant_is_not_a_guard_conflict(self):
        # `grep` is not `git grep`, and no guard blocks it. It is also
        # acceptable to the safety floor by design (see firm_core's
        # NO_BARE_WILDCARD note) — so nothing should flag it.
        self.assertIsNone(allowlist.guard_conflict("Bash(grep:*)"))
        self.assertIsNone(self._solo("Bash(grep:*)"))

    def test_a_compound_grant_is_not_flagged(self):
        # The compound guard takes a `#compound-ok` marker, so a rule granting
        # a compound is not in conflict with it. Flagging those would bury the
        # real finding in noise.
        self.assertIsNone(allowlist.guard_conflict("Bash(ls && pwd:*)"))

    def test_non_bash_rules_are_ignored(self):
        self.assertIsNone(allowlist.guard_conflict("Read(/Users/me/x/**)"))
        self.assertIsNone(allowlist.guard_conflict("not a rule at all"))

    def test_it_precedes_the_path_verdict(self):
        # The live entry carries an absolute worktree path, so a
        # machine-path/stale verdict would otherwise mask the real finding.
        rule = "Bash(git -C /Users/nobody/definitely-not-here/* grep:*)"
        self.assertEqual(self._solo(rule)[0], "guard-conflict")

    def test_a_missing_guard_degrades_to_no_finding(self):
        # An audit tool must report what it can, not refuse to run because a
        # hook was moved or renamed.
        with mock.patch.object(allowlist, "_load_guard", return_value=None):
            self.assertIsNone(allowlist.guard_conflict("Bash(git grep:*)"))

    def test_the_shortlist_carries_it_with_its_own_category(self):
        allow = ["Bash(make lint:*)", "Bash(git grep:*)"]
        out = cruft(allow)
        self.assertEqual([f["index"] for f in out["flagged"]], [1])
        self.assertEqual(out["flagged"][0]["category"], "guard-conflict")


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

    def test_refuses_a_bare_verb_wildcard_the_fast_firm_would_refuse(self):
        """`firm_into` has no floor of its own — `firm_last` checks it in the
        caller — so a write path that skipped the check would grant exactly what
        the fast firm refuses, via one non-prompting pre-approved call."""
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
