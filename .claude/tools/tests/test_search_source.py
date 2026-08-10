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
