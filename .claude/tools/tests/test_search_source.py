"""Stdlib ``unittest`` tests for the source-search wrapper.

Built over a small throwaway tree, so the pruning rules — generated families,
never-search dirs, symlinks, oversized blobs — are asserted against real
directory walks rather than mocked out. Run via the repo's ``make tools-tests``.
"""

from __future__ import annotations

import io
import os
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import search_source as ss


class ExcludeListTests(unittest.TestCase):
    """The exclusions come from review_diff, so there is one owner."""

    def test_generated_dirs_are_pruned_by_basename(self):
        names = ss.excluded_dir_names()
        self.assertIn("generated", names)

    def test_never_search_dirs_are_pruned(self):
        names = ss.excluded_dir_names()
        for name in ("target", "node_modules", ".git"):
            self.assertIn(name, names)

    def test_file_shaped_generated_families_are_skipped(self):
        names = ss.excluded_file_names()
        self.assertIn("Cargo.lock", names)
        self.assertIn("pnpm-lock.yaml", names)
        self.assertNotIn("generated", names)

    def test_the_worktrees_tree_is_pruned(self):
        """Each live worktree is a full checkout of this same repo, so
        searching from the base repo would return every match once per
        worktree."""
        self.assertIn("worktrees", ss.excluded_dir_names())


class ClipTests(unittest.TestCase):
    def test_a_normal_line_is_untouched(self):
        line = 'fn main() { println!("hi"); }'
        self.assertEqual(ss.clip(line), line)

    def test_a_very_long_line_is_truncated_and_says_so(self):
        line = "data:image/png;base64," + ("A" * 5000)
        got = ss.clip(line)
        self.assertLess(len(got), 500)
        self.assertIn("…", got)
        self.assertIn("chars]", got)

    def test_truncation_keeps_the_start_so_the_match_stays_identifiable(self):
        line = "SENTINEL" + ("x" * 5000)
        self.assertTrue(ss.clip(line).startswith("SENTINEL"))


class MergeContextBlockTests(unittest.TestCase):
    def _match(self, path, start, lines):
        return {"path": path, "context_start": start, "context": lines}

    def test_overlapping_windows_in_one_file_become_one_block(self):
        matches = [
            self._match("a.rs", 10, ["l10", "l11", "l12", "l13"]),
            self._match("a.rs", 12, ["l12", "l13", "l14", "l15"]),
        ]
        blocks = ss.merge_context_blocks(matches)
        self.assertEqual(len(blocks), 1)
        path, start, lines = blocks[0]
        self.assertEqual((path, start), ("a.rs", 10))
        self.assertEqual(lines, ["l10", "l11", "l12", "l13", "l14", "l15"])

    def test_adjacent_windows_merge_too(self):
        matches = [
            self._match("a.rs", 1, ["l1", "l2"]),
            self._match("a.rs", 3, ["l3", "l4"]),
        ]
        blocks = ss.merge_context_blocks(matches)
        self.assertEqual(len(blocks), 1)
        self.assertEqual(blocks[0][2], ["l1", "l2", "l3", "l4"])

    def test_a_fully_contained_window_adds_nothing(self):
        matches = [
            self._match("a.rs", 10, ["l10", "l11", "l12", "l13"]),
            self._match("a.rs", 11, ["l11", "l12"]),
        ]
        blocks = ss.merge_context_blocks(matches)
        self.assertEqual(len(blocks), 1)
        self.assertEqual(blocks[0][2], ["l10", "l11", "l12", "l13"])

    def test_distant_windows_stay_separate(self):
        matches = [
            self._match("a.rs", 1, ["l1"]),
            self._match("a.rs", 50, ["l50"]),
        ]
        self.assertEqual(len(ss.merge_context_blocks(matches)), 2)

    def test_windows_in_different_files_never_merge(self):
        matches = [
            self._match("a.rs", 10, ["l10", "l11"]),
            self._match("b.rs", 10, ["l10", "l11"]),
        ]
        blocks = ss.merge_context_blocks(matches)
        self.assertEqual([b[0] for b in blocks], ["a.rs", "b.rs"])


class SearchTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def write(self, rel, text):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def test_finds_a_match_with_path_and_line(self):
        self.write("programs/src/swap.rs", "fn a() {}\n// WARNING 1e guard\n")
        out = ss.search("WARNING 1", self.root)
        self.assertEqual(out["total"], 1)
        self.assertEqual(out["matches"][0]["path"], "programs/src/swap.rs")
        self.assertEqual(out["matches"][0]["line"], 2)
        self.assertIn("WARNING 1e", out["matches"][0]["text"])

    def test_finds_the_same_label_in_two_files(self):
        """The uniqueness sweep this tool exists for: a numbered guard label
        reused in an unrelated file is exactly what one grep caught."""
        self.write("programs/src/a.rs", "// WARNING 1e first\n")
        self.write("sdk/rs/src/b.rs", "// WARNING 1e second\n")
        out = ss.search("WARNING 1e", self.root)
        self.assertEqual(out["total"], 2)
        self.assertEqual(len(out["files"]), 2)

    def test_prunes_a_generated_tree(self):
        self.write("sdk/ts/src/generated/instructions.ts", "export const swap = 1;\n")
        self.write("sdk/ts/src/index.ts", "export const swap = 2;\n")
        out = ss.search("swap", self.root)
        self.assertEqual([m["path"] for m in out["matches"]], ["sdk/ts/src/index.ts"])

    def test_prunes_target_and_node_modules(self):
        self.write("target/debug/build.rs", "fn needle() {}\n")
        self.write("node_modules/pkg/index.js", "var needle = 1;\n")
        self.write("tui/src/ui.rs", "fn needle() {}\n")
        out = ss.search("needle", self.root)
        self.assertEqual([m["path"] for m in out["matches"]], ["tui/src/ui.rs"])

    def test_skips_the_lock_files(self):
        self.write("Cargo.lock", 'name = "needle"\n')
        self.write("Cargo.toml", 'name = "needle"\n')
        out = ss.search("needle", self.root)
        self.assertEqual([m["path"] for m in out["matches"]], ["Cargo.toml"])

    def test_default_extensions_skip_markdown(self):
        """A symbol sweep shouldn't drag prose in — that noise is what made the
        unscoped hoisted grep expensive."""
        self.write("docs/notes.md", "the needle in prose\n")
        self.write("tui/src/ui.rs", "fn needle() {}\n")
        out = ss.search("needle", self.root)
        self.assertEqual([m["path"] for m in out["matches"]], ["tui/src/ui.rs"])

    def test_all_text_includes_markdown(self):
        self.write("docs/notes.md", "the needle in prose\n")
        out = ss.search("needle", self.root, extensions=None)
        self.assertEqual([m["path"] for m in out["matches"]], ["docs/notes.md"])

    def test_esm_and_cjs_configs_are_source(self):
        """Their absence was a silent under-report: a sweep for a symbol defined
        in a `.mjs` build script came back with a confident zero."""
        self.write("decks/scripts/capture.mjs", "const CAPTURE_SCALE = 3;\n")
        self.write("cfg/legacy.cjs", "module.exports = { CAPTURE_SCALE: 3 };\n")
        out = ss.search("CAPTURE_SCALE", self.root)
        self.assertEqual(
            [m["path"] for m in out["matches"]],
            ["cfg/legacy.cjs", "decks/scripts/capture.mjs"],
        )

    def test_an_empty_defaulted_run_flags_that_prose_was_not_searched(self):
        """The difference between a real negative and an unasked question."""
        self.write("docs/notes.md", "the needle in prose\n")
        out = ss.search("needle", self.root)
        self.assertEqual(out["total"], 0)
        self.assertTrue(out["narrowed_by_default"])

    def test_a_partial_hit_is_still_flagged_as_defaulted(self):
        """A non-empty result that silently skipped every .md is MORE
        misleading than an empty one — it reads as a complete answer. This
        shape produced a wrong 'referenced by nothing' conclusion about a
        tool that three .md files reference."""
        self.write("tui/src/ui.rs", "fn needle() {}\n")
        self.write("docs/notes.md", "the needle in prose\n")
        out = ss.search("needle", self.root)
        self.assertEqual(out["total"], 1)  # only the .rs hit
        self.assertTrue(out["narrowed_by_default"])

    def test_the_partial_warning_reaches_the_summary_line(self):
        self.write("tui/src/ui.rs", "fn needle() {}\n")
        self.write("docs/notes.md", "the needle in prose\n")
        out = ss.search("needle", self.root)
        err = io.StringIO()
        with redirect_stdout(io.StringIO()), redirect_stderr(err):
            ss.print_result(out, files_only=True, context=0)
        self.assertIn("may be partial", err.getvalue())

    def test_an_explicit_extension_set_is_not_flagged_as_defaulted(self):
        """`--ext md` found nothing means nothing is there — don't second-guess
        an answer the caller already scoped."""
        self.write("tui/src/ui.rs", "fn needle() {}\n")
        out = ss.search("needle", self.root, extensions=("md",))
        self.assertEqual(out["total"], 0)
        self.assertFalse(out["narrowed_by_default"])

    def test_a_globbed_run_is_not_flagged_as_defaulted(self):
        """Under a glob the default resolves to *every* extension, so nothing
        was narrowed and there is nothing to warn about."""
        self.write("docs/notes.md", "prose without the symbol\n")
        out = ss.search("needle", self.root, globs=("docs/notes.md",))
        self.assertEqual(out["total"], 0)
        self.assertFalse(out["narrowed_by_default"])

    def test_explicit_extension_filter(self):
        self.write("a.rs", "needle\n")
        self.write("b.ts", "needle\n")
        out = ss.search("needle", self.root, extensions=("rs",))
        self.assertEqual([m["path"] for m in out["matches"]], ["a.rs"])

    def test_dir_scoping(self):
        self.write("programs/a.rs", "needle\n")
        self.write("tui/b.rs", "needle\n")
        out = ss.search("needle", self.root, dirs=["programs"])
        self.assertEqual([m["path"] for m in out["matches"]], ["programs/a.rs"])

    def test_missing_dir_errors(self):
        with self.assertRaises(ss.SearchSourceError):
            ss.search("x", self.root, dirs=["nope"])

    def test_an_absolute_dir_outside_the_root_is_refused(self):
        """`Path("/repo") / "/etc"` is `/etc`, so without containment an absolute
        --dir searches outside the tree and prints its matching lines — behind a
        single blanket allow-rule."""
        with self.assertRaises(ss.SearchSourceError):
            ss.search("x", self.root, dirs=["/etc"])

    def test_a_dot_dot_dir_outside_the_root_is_refused(self):
        with self.assertRaises(ss.SearchSourceError):
            ss.search("x", self.root, dirs=["../.."])

    def test_a_bad_root_errors_rather_than_reporting_zero(self):
        """A wrong --root would otherwise be silent: iter_files swallows the
        iterdir error and the run prints "0 match(es)" — a "searched everything,
        found nothing" in the one tool whose thesis is not doing that."""
        with self.assertRaises(ss.SearchSourceError):
            ss.search("x", self.root / "does-not-exist")

    def test_nested_dirs_do_not_double_count(self):
        self.write("programs/src/a.rs", "needle\n")
        out = ss.search("needle", self.root, dirs=["programs", "programs/src"])
        self.assertEqual(out["total"], 1)
        self.assertEqual(out["files"], ["programs/src/a.rs"])

    def test_repeated_dirs_do_not_double_count(self):
        self.write("programs/a.rs", "needle\n")
        out = ss.search("needle", self.root, dirs=["programs", "programs"])
        self.assertEqual(out["total"], 1)

    def test_an_oversized_skip_is_reported_not_silent(self):
        """The size cap is the tool's other cap, and the same rule applies."""
        big = self.write("big.rs", "")
        big.write_text("needle\n" * 400_000, encoding="utf-8")
        out = ss.search("needle", self.root)
        self.assertEqual(out["total"], 0)
        self.assertEqual(out["skipped_oversized"], ["big.rs"])

    def test_nothing_skipped_reports_an_empty_list(self):
        self.write("a.rs", "needle\n")
        self.assertEqual(ss.search("needle", self.root)["skipped_oversized"], [])

    def test_regex_by_default(self):
        self.write("a.rs", "fn compute_fill() {}\n")
        out = ss.search(r"fn compute_\w+", self.root)
        self.assertEqual(out["total"], 1)

    def test_fixed_treats_the_pattern_literally(self):
        self.write("a.rs", "value[0] = 1;\n")
        out = ss.search("value[0]", self.root, fixed=True)
        self.assertEqual(out["total"], 1)

    def test_a_bad_regex_errors_with_the_fixed_hint(self):
        with self.assertRaises(ss.SearchSourceError) as ctx:
            ss.search("value[0", self.root)
        self.assertIn("--fixed", str(ctx.exception))

    def test_ignore_case(self):
        self.write("a.rs", "// Needle\n")
        self.assertEqual(ss.search("needle", self.root)["total"], 0)
        self.assertEqual(ss.search("needle", self.root, ignore_case=True)["total"], 1)

    def test_context_lines(self):
        self.write("a.rs", "one\ntwo\nneedle\nfour\nfive\n")
        out = ss.search("needle", self.root, context=1)
        self.assertEqual(out["matches"][0]["context"], ["two", "needle", "four"])
        self.assertEqual(out["matches"][0]["context_start"], 2)

    def test_cap_reports_what_it_dropped(self):
        """A silent cap reads as "searched everything" — it must not be silent."""
        self.write("a.rs", "needle\n" * 10)
        out = ss.search("needle", self.root, limit=3)
        self.assertEqual(out["total"], 10)
        self.assertEqual(len(out["matches"]), 3)
        self.assertEqual(out["truncated"], 7)

    def test_no_truncation_when_under_the_cap(self):
        self.write("a.rs", "needle\n")
        out = ss.search("needle", self.root, limit=10)
        self.assertEqual(out["truncated"], 0)

    def test_results_are_path_then_line_sorted(self):
        self.write("b.rs", "needle\n")
        self.write("a.rs", "x\nneedle\nneedle\n")
        out = ss.search("needle", self.root)
        self.assertEqual(
            [(m["path"], m["line"]) for m in out["matches"]],
            [("a.rs", 2), ("a.rs", 3), ("b.rs", 1)],
        )

    def test_symlinks_are_not_followed(self):
        """A link into a pruned tree must not smuggle it back in."""
        self.write("target/debug/x.rs", "needle\n")
        link = self.root / "shortcut"
        os.symlink(self.root / "target", link)
        out = ss.search("needle", self.root)
        self.assertEqual(out["total"], 0)

    def test_oversized_file_is_skipped(self):
        big = self.write("big.rs", "")
        big.write_text("needle\n" * 400_000, encoding="utf-8")
        self.assertGreater(big.stat().st_size, ss.MAX_FILE_BYTES)
        out = ss.search("needle", self.root)
        self.assertEqual(out["total"], 0)

    def test_no_match_is_empty_not_an_error(self):
        self.write("a.rs", "nothing here\n")
        out = ss.search("needle", self.root)
        self.assertEqual(out["total"], 0)
        self.assertEqual(out["files"], [])


class DirIsADirectoryTests(unittest.TestCase):
    """``--dir`` given a file used to answer a confident zero.

    The file cleared `exists()`, became a walk root, and `iter_files` swallowed
    the resulting `iterdir` OSError — so the caller got `0 match(es)` and read it
    as absence. Silent-wrong-answer class, not merely a wasted call.
    """

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def write(self, rel, text):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def test_a_file_passed_to_dir_is_refused_not_answered_with_zero(self):
        self.write("tui/src/ui.rs", "fn needle() {}\n")
        with self.assertRaises(ss.SearchSourceError) as caught:
            ss.search("needle", self.root, dirs=["tui/src/ui.rs"])
        message = str(caught.exception)
        self.assertIn("takes a directory", message)
        # The error has to name the flag that does what was meant, or the reader
        # is left with a refusal and no next move.
        self.assertIn("--glob", message)

    def test_the_glob_the_error_recommends_actually_works(self):
        self.write("tui/src/ui.rs", "fn needle() {}\n")
        out = ss.search("needle", self.root, globs=("tui/src/ui.rs",))
        self.assertEqual(out["total"], 1)

    def test_a_real_directory_is_unaffected(self):
        self.write("tui/src/ui.rs", "fn needle() {}\n")
        self.assertEqual(ss.search("needle", self.root, dirs=["tui/src"])["total"], 1)

    def test_a_missing_dir_still_reports_the_missing_path(self):
        with self.assertRaises(ss.SearchSourceError) as caught:
            ss.search("needle", self.root, dirs=["nope"])
        self.assertIn("no such directory", str(caught.exception))


class ContextNudgeTests(unittest.TestCase):
    """The summary should say when `--context` was probably the wrong shape.

    Nothing here is a wrong answer — these results are complete. The nudge exists
    because the narrowness rule is missed while typing, not while reading.
    """

    def _summary(self, result, files_only=False, context=2):
        err = io.StringIO()
        # stdout is captured too, not just redirected for tidiness: `--files-only`
        # prints the path list there, and letting it escape scribbles over the
        # test runner's own output.
        with redirect_stderr(err), redirect_stdout(io.StringIO()):
            ss.print_result(result, files_only, context)
        return err.getvalue()

    def _result(self, total, files):
        return {
            "matches": [],
            "files": [f"f{k}.rs" for k in range(files)],
            "total": total,
            "truncated": 0,
        }

    def test_context_over_many_files_suggests_files_only(self):
        got = self._summary(self._result(20, ss.CONTEXT_FILE_NUDGE + 1))
        self.assertIn("--files-only", got)
        self.assertIn("WHERE", got)

    def test_context_over_a_handful_of_files_says_nothing(self):
        got = self._summary(self._result(5, ss.CONTEXT_FILE_NUDGE))
        self.assertNotIn("NOTE: --context", got)

    def test_matches_clustered_in_one_file_suggest_a_slice_read(self):
        got = self._summary(self._result(ss.CONTEXT_DENSITY_NUDGE, 1))
        self.assertIn("cluster in one file", got)
        self.assertIn("slice Read", got)

    def test_a_few_matches_in_one_file_say_nothing(self):
        got = self._summary(self._result(2, 1))
        self.assertNotIn("cluster in", got)

    def test_dense_matches_across_a_handful_of_files_also_nudge(self):
        # The gap the single-file test left: 2-3 files fired neither branch,
        # however dense, yet that is the same overlap shape.
        got = self._summary(self._result(40, 3))
        self.assertIn("cluster in 3 files", got)

    def test_no_nudge_without_context(self):
        got = self._summary(self._result(50, 20), context=0)
        self.assertNotIn("NOTE: --context", got)

    def test_no_nudge_when_already_files_only(self):
        got = self._summary(self._result(50, 20), files_only=True)
        self.assertNotIn("NOTE: --context", got)

    def test_no_nudge_on_an_empty_result(self):
        # An empty result's problem is scope, not output form; the existing
        # prose/glob diagnostics own that case.
        got = self._summary(self._result(0, 0))
        self.assertNotIn("NOTE: --context", got)


class GlobFilterTests(unittest.TestCase):
    """``--glob`` picks *files*, where ``--dir`` picks subtrees.

    The measured gap: a section map of three named docs cost 3.0k because
    ``--dir docs --ext md`` returned ~200 headings across 18 files.
    """

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def write(self, rel, text):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def test_an_exact_path_selects_only_that_file(self):
        self.write("docs/fx-survey.md", "# Wanted\n")
        self.write("docs/architecture.md", "# Unwanted\n")
        out = ss.search("^# ", self.root, globs=("docs/fx-survey.md",))
        self.assertEqual(out["files"], ["docs/fx-survey.md"])

    def test_several_named_paths_select_exactly_those(self):
        self.write("docs/a.md", "# A\n")
        self.write("docs/b.md", "# B\n")
        self.write("docs/c.md", "# C\n")
        out = ss.search("^# ", self.root, globs=("docs/a.md", "docs/b.md"))
        self.assertEqual(out["files"], ["docs/a.md", "docs/b.md"])

    def test_a_single_star_does_not_cross_a_separator(self):
        """`fnmatch` maps `*` to `.*`, which would wrongly match the nested file."""
        self.write("docs/top.md", "# Top\n")
        self.write("docs/deep/nested.md", "# Nested\n")
        out = ss.search("^# ", self.root, globs=("docs/*.md",))
        self.assertEqual(out["files"], ["docs/top.md"])

    def test_a_double_star_spans_depth(self):
        self.write("programs/a/state.rs", "struct S;\n")
        self.write("programs/a/b/state.rs", "struct S;\n")
        self.write("programs/a/swap.rs", "struct S;\n")
        out = ss.search("struct S", self.root, globs=("programs/**/state.rs",))
        self.assertEqual(out["files"], ["programs/a/b/state.rs", "programs/a/state.rs"])

    def test_a_double_star_also_matches_at_zero_depth(self):
        """`a/**/b` should match `a/b`, not only `a/x/b`."""
        self.write("programs/state.rs", "struct S;\n")
        out = ss.search("struct S", self.root, globs=("programs/**/state.rs",))
        self.assertEqual(out["files"], ["programs/state.rs"])

    def test_a_separator_free_pattern_matches_on_basename(self):
        self.write("a/Cargo.toml", "[package]\n")
        self.write("b/deep/Cargo.toml", "[package]\n")
        out = ss.search(r"\[package\]", self.root, globs=("*.toml",))
        self.assertEqual(out["files"], ["a/Cargo.toml", "b/deep/Cargo.toml"])

    def test_a_mid_segment_double_star_does_not_span_separators(self):
        """`docs/fx**.md` is not a globstar — treating it as one would
        contradict the "stops at a separator" promise. Bash, gitignore and
        pathlib all degrade a non-boundary `**` to a single `*`."""
        self.write("docs/fx.md", "# Flat\n")
        self.write("docs/fx/deep/notes.md", "# Nested\n")
        out = ss.search("^# ", self.root, globs=("docs/fx**.md",))
        self.assertEqual(out["files"], ["docs/fx.md"])

    def test_a_trailing_double_star_still_spans(self):
        self.write("docs/a.md", "# A\n")
        self.write("docs/deep/b.md", "# B\n")
        out = ss.search("^# ", self.root, globs=("docs/**",))
        self.assertEqual(out["files"], ["docs/a.md", "docs/deep/b.md"])

    def test_a_leading_double_star_spans(self):
        self.write("state.rs", "struct S;\n")
        self.write("a/b/state.rs", "struct S;\n")
        out = ss.search("struct S", self.root, globs=("**/state.rs",))
        self.assertEqual(out["files"], ["a/b/state.rs", "state.rs"])

    def test_regex_metacharacters_in_a_pattern_are_literal(self):
        """Unescaped, `a+b` is a regex quantifier that would also match `ab.rs`."""
        self.write("a+b.rs", "fn x() {}\n")
        self.write("ab.rs", "fn x() {}\n")
        out = ss.search("fn x", self.root, globs=("a+b.rs",))
        self.assertEqual(out["files"], ["a+b.rs"])

    def test_oversized_skips_are_scoped_to_the_glob(self):
        """A `--glob docs/*.md` run must not report skipping a huge binary it
        never intended to search — the glob filter runs before the size check."""
        self.write("docs/small.md", "# ok\n")
        big = self.root / "huge.bin"
        big.write_text("x" * (ss.MAX_FILE_BYTES + 10), encoding="utf-8")
        out = ss.search("ok", self.root, globs=("docs/*.md",))
        self.assertEqual(out["skipped_oversized"], [])

    def test_an_ext_drop_is_distinguished_from_a_glob_miss(self):
        """Both search zero files, but they need different fixes — one is a
        typo'd path, the other an extension filter."""
        self.write("docs/real.md", "# Real\n")

        missed = ss.search("^# ", self.root, globs=("docs/typo.md",))
        self.assertEqual(missed["glob_hits"], 0)
        self.assertEqual(missed["scanned"], 0)

        filtered = ss.search(
            "^# ", self.root, globs=("docs/real.md",), extensions=("rs",)
        )
        self.assertEqual(filtered["glob_hits"], 1)
        self.assertEqual(filtered["scanned"], 0)

    def test_a_glob_cannot_reach_into_a_pruned_tree(self):
        """The exclude lists still win: a glob is a narrowing, not an override."""
        self.write("target/debug/notes.md", "# Generated\n")
        out = ss.search("^# ", self.root, globs=("target/**/notes.md",))
        self.assertEqual(out["files"], [])

    def test_scanned_reports_zero_when_the_glob_named_nothing(self):
        """ "Glob matched no files" and "its files held no match" both total 0."""
        self.write("docs/real.md", "# Real\n")
        out = ss.search("^# ", self.root, globs=("docs/typo.md",))
        self.assertEqual(out["total"], 0)
        self.assertEqual(out["scanned"], 0)
        self.assertTrue(out["globbed"])

        found = ss.search("absent", self.root, globs=("docs/real.md",))
        self.assertEqual(found["total"], 0)
        self.assertEqual(found["scanned"], 1)

    def test_globs_compose_with_dirs(self):
        self.write("docs/a.md", "# A\n")
        self.write("spec/a.md", "# A\n")
        out = ss.search("^# ", self.root, dirs=["docs"], globs=("*.md",))
        self.assertEqual(out["files"], ["docs/a.md"])


class CliTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        (self.root / "a.rs").write_text("fn needle() {}\n", encoding="utf-8")

    def _capture(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = ss.run(argv)
        return code, out.getvalue(), err.getvalue()

    def test_prints_grep_shaped_lines(self):
        code, out, err = self._capture(
            ["search_source.py", "needle", "--root", str(self.root)]
        )
        self.assertEqual(code, 0)
        self.assertEqual(out.strip(), "a.rs:1:fn needle() {}")
        self.assertIn("1 match(es) in 1 file(s)", err)

    def test_files_only(self):
        _, out, _ = self._capture(
            ["search_source.py", "needle", "--root", str(self.root), "--files-only"]
        )
        self.assertEqual(out.strip(), "a.rs")

    def test_exits_one_when_nothing_matched(self):
        code, out, _ = self._capture(
            ["search_source.py", "absent", "--root", str(self.root)]
        )
        self.assertEqual(code, 1)
        self.assertEqual(out.strip(), "")

    def test_an_empty_default_run_says_prose_was_not_searched(self):
        """One measured run read this bare zero as absence and fell back to a
        bare `grep` — the very thing this tool replaces."""
        (self.root / "notes.md").write_text("the absent one\n", encoding="utf-8")
        code, _, err = self._capture(
            ["search_source.py", "absent", "--root", str(self.root)]
        )
        self.assertEqual(code, 1)
        self.assertIn("searched the source set only", err)
        self.assertIn("--ext md", err)

    def test_an_explicitly_scoped_empty_run_stays_quiet(self):
        code, _, err = self._capture(
            ["search_source.py", "absent", "--root", str(self.root), "--ext", "md"]
        )
        self.assertEqual(code, 1)
        self.assertNotIn("searched the source set only", err)

    def test_a_run_that_matched_does_not_nag_about_extensions(self):
        _, _, err = self._capture(
            ["search_source.py", "needle", "--root", str(self.root)]
        )
        self.assertNotIn("searched the source set only", err)

    def test_glob_searches_every_extension_by_default(self):
        """A named `.md` must not be dropped by the source-extension heuristic —
        that is the silent under-report the tool exists to avoid."""
        (self.root / "notes.md").write_text("# needle\n", encoding="utf-8")
        code, out, _ = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--glob",
                "notes.md",
            ]
        )
        self.assertEqual(code, 0)
        self.assertEqual(out.strip(), "notes.md:1:# needle")

    def test_an_explicit_ext_still_wins_over_the_glob_default(self):
        (self.root / "notes.md").write_text("needle\n", encoding="utf-8")
        code, _, _ = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--glob",
                "*.md",
                "--ext",
                "rs",
            ]
        )
        self.assertEqual(code, 1)

    def test_a_repeated_ext_accumulates_rather_than_the_last_one_winning(self):
        """The filed defect: argparse's default `store` kept only the last
        `--ext`, so the two-flag form searched `tsx` alone and reported a clean
        `0 match(es)` — a false negative indistinguishable from a true one."""
        (self.root / "a.ts").write_text("needle\n", encoding="utf-8")
        (self.root / "b.tsx").write_text("needle\n", encoding="utf-8")

        _, one, _ = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--ext",
                "ts",
                "--files-only",
            ]
        )
        _, both, _ = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--ext",
                "ts",
                "--ext",
                "tsx",
                "--files-only",
            ]
        )
        self.assertEqual(set(one.split()), {"a.ts"})
        # The whole point: two flags is a superset of one, never a replacement.
        self.assertTrue(set(one.split()) <= set(both.split()))
        self.assertEqual(set(both.split()), {"a.ts", "b.tsx"})

    def test_a_repeated_ext_matches_the_comma_separated_spelling(self):
        (self.root / "a.ts").write_text("needle\n", encoding="utf-8")
        (self.root / "b.tsx").write_text("needle\n", encoding="utf-8")
        base = ["search_source.py", "needle", "--root", str(self.root), "--files-only"]
        _, repeated, _ = self._capture(base + ["--ext", "ts", "--ext", "tsx"])
        _, comma, _ = self._capture(base + ["--ext", "ts,tsx"])
        self.assertEqual(set(repeated.split()), set(comma.split()))

    def test_a_repeated_glob_accumulates(self):
        (self.root / "a.ts").write_text("needle\n", encoding="utf-8")
        (self.root / "b.tsx").write_text("needle\n", encoding="utf-8")
        base = ["search_source.py", "needle", "--root", str(self.root), "--files-only"]
        _, one, _ = self._capture(base + ["--glob", "a.ts"])
        _, both, _ = self._capture(base + ["--glob", "a.ts", "--glob", "b.tsx"])
        self.assertEqual(set(one.split()), {"a.ts"})
        self.assertEqual(set(both.split()), {"a.ts", "b.tsx"})

    def test_a_repeated_dir_accumulates(self):
        (self.root / "one").mkdir()
        (self.root / "two").mkdir()
        (self.root / "one" / "a.rs").write_text("needle\n", encoding="utf-8")
        (self.root / "two" / "b.rs").write_text("needle\n", encoding="utf-8")
        base = ["search_source.py", "needle", "--root", str(self.root), "--files-only"]
        _, one, _ = self._capture(base + ["--dir", "one"])
        _, both, _ = self._capture(base + ["--dir", "one", "--dir", "two"])
        self.assertEqual(set(one.split()), {"one/a.rs"})
        self.assertEqual(set(both.split()), {"one/a.rs", "two/b.rs"})

    def test_a_repeated_value_is_not_searched_twice(self):
        """`--ext ts --ext ts,tsx` is a plausible thing to type."""
        (self.root / "a.ts").write_text("needle\n", encoding="utf-8")
        _, _, err = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--ext",
                "ts",
                "--ext",
                "ts,tsx",
            ]
        )
        self.assertIn("1 match(es) in 1 file(s)", err)

    def test_an_empty_ext_is_refused_not_silently_widened(self):
        """Symmetric with `--glob ''`: falling back to the default set on an
        empty `--ext` silently answers a question the caller did not ask."""
        with self.assertRaises(ss.SearchSourceError):
            self._capture(
                ["search_source.py", "needle", "--root", str(self.root), "--ext", ""]
            )

    def test_an_empty_dir_is_refused_not_silently_widened(self):
        with self.assertRaises(ss.SearchSourceError):
            self._capture(
                ["search_source.py", "needle", "--root", str(self.root), "--dir", ""]
            )

    def test_a_glob_matching_nothing_warns_rather_than_reading_as_a_negative(self):
        _, _, err = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--glob",
                "does/not/exist.rs",
            ]
        )
        self.assertIn("--glob matched no files", err)

    def test_an_empty_glob_is_refused(self):
        with self.assertRaises(ss.SearchSourceError):
            self._capture(
                ["search_source.py", "needle", "--root", str(self.root), "--glob", ","]
            )

    def test_an_empty_string_glob_is_refused_not_silently_dropped(self):
        """`--glob ''` is falsy, so a truthiness test would skip the filter
        entirely and sweep the whole tree — the broad result --glob prevents."""
        with self.assertRaises(ss.SearchSourceError):
            self._capture(
                ["search_source.py", "needle", "--root", str(self.root), "--glob", ""]
            )

    def test_an_ext_drop_under_a_glob_says_so_rather_than_blaming_the_path(self):
        (self.root / "notes.md").write_text("needle\n", encoding="utf-8")
        _, _, err = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--glob",
                "*.md",
                "--ext",
                "rs",
            ]
        )
        self.assertIn("--ext excluded all of them", err)
        self.assertNotIn("--glob matched no files", err)

    def test_a_glob_naming_a_pruned_path_says_so_rather_than_blaming_the_path(self):
        """`--glob sdk/idl/dropset.json` named a file that is really there; the
        old message ("matched no files") sent the reader to re-check a path
        sitting in plain sight, when the answer is that it is excluded."""
        (self.root / "Cargo.lock").write_text("needle\n", encoding="utf-8")
        _, _, err = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--glob",
                "Cargo.lock",
                "--all-text",
            ]
        )
        self.assertIn("excluded as a generated family", err)
        self.assertIn("Cargo.lock", err)
        self.assertNotIn("--glob matched no files", err)

    def test_a_pruned_glob_is_reported_even_when_another_glob_matched(self):
        """The partial case, and the more dangerous one. Because globs now
        accumulate, `--glob live --glob pruned` is the encouraged spelling —
        and there the run returns results and reads as complete while a path
        the caller named by hand was never searched at all."""
        (self.root / "Cargo.lock").write_text("needle\n", encoding="utf-8")
        code, out, err = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--glob",
                "a.rs",
                "--glob",
                "Cargo.lock",
                "--all-text",
                "--files-only",
            ]
        )
        # The live glob still returns its match...
        self.assertEqual(code, 0)
        self.assertEqual(set(out.split()), {"a.rs"})
        # ...and the pruned one is named rather than silently dropped.
        self.assertIn("excluded as a generated family", err)
        self.assertIn("Cargo.lock", err)

    def test_a_pruned_glob_survives_the_ext_excluded_everything_branch(self):
        """The last corner the nested form hid it in: another glob matched, but
        --ext filtered out every file it matched, so the run reports the --ext
        mismatch and nothing was searched — while the path the caller named by
        hand goes unmentioned."""
        (self.root / "Cargo.lock").write_text("needle\n", encoding="utf-8")
        (self.root / "notes.md").write_text("needle\n", encoding="utf-8")
        _, _, err = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--glob",
                "Cargo.lock",
                "--glob",
                "notes.md",
                "--ext",
                "rs",
            ]
        )
        self.assertIn("--ext excluded all of them", err)
        self.assertIn("excluded as a generated family", err)
        self.assertIn("Cargo.lock", err)

    def test_no_pruned_warning_when_every_glob_was_searchable(self):
        _, _, err = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--glob",
                "a.rs",
                "--files-only",
            ]
        )
        self.assertNotIn("excluded as a generated family", err)

    def test_a_glob_naming_a_genuinely_absent_path_still_blames_the_path(self):
        _, _, err = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--glob",
                "Cargo.lock",
                "--all-text",
            ]
        )
        self.assertIn("--glob matched no files", err)
        self.assertNotIn("excluded as a generated family", err)

    def test_truncation_is_announced_on_stderr(self):
        (self.root / "many.rs").write_text("needle\n" * 5, encoding="utf-8")
        _, _, err = self._capture(
            ["search_source.py", "needle", "--root", str(self.root), "--max", "2"]
        )
        self.assertIn("NOT shown", err)

    def test_context_renumbers_and_separates(self):
        """The documented headline usage. Exercises print_result's context branch,
        not just the data search() returns."""
        (self.root / "ctx.rs").write_text(
            "one\ntwo\nneedle\nfour\nfive\n", encoding="utf-8"
        )
        _, out, _ = self._capture(
            [
                "search_source.py",
                "needle",
                "--root",
                str(self.root),
                "--context",
                "1",
            ]
        )
        self.assertIn("ctx.rs:2:two", out)
        self.assertIn("ctx.rs:3:needle", out)
        self.assertIn("ctx.rs:4:four", out)
        self.assertIn("--", out)

    def test_oversized_skip_is_announced_on_stderr(self):
        big = self.root / "big.rs"
        big.write_text("needle\n" * 400_000, encoding="utf-8")
        _, _, err = self._capture(
            ["search_source.py", "needle", "--root", str(self.root)]
        )
        self.assertIn("skipped as oversized", err)

    def test_ext_and_all_text_are_alternatives(self):
        with self.assertRaises(ss.SearchSourceError):
            ss.run(
                [
                    "search_source.py",
                    "needle",
                    "--root",
                    str(self.root),
                    "--ext",
                    "rs",
                    "--all-text",
                ]
            )


if __name__ == "__main__":
    unittest.main()
