#!/usr/bin/env python3
"""Stdlib ``unittest`` tests for ``review_diff.py``.

The pure logic — pattern matching, ``--numstat`` parsing, and the gate/verdict
assembly over a real throwaway git repo — is what these cover. Run via the
repo's ``make tools-tests``.
"""

from __future__ import annotations

import io
import json
import os
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

import review_diff as rd


def _repo_root() -> Path:
    """The repo root, from this test file's own location.

    Resolved from ``__file__`` rather than the cwd so the parity test below reads
    the real workflow whether it is run via ``make tools-tests`` from the root or
    by pointing unittest at this file directly.
    """
    return Path(__file__).resolve().parents[3]


def workflow_code_excludes(text: str) -> list[str]:
    """The negated paths of the ``code`` filter in the Tests workflow.

    A deliberately small hand parser rather than a YAML dependency: these tools
    are stdlib-only, and the shape being read is three lines of literal list
    syntax. It takes the ``code:`` block and collects every ``- '!…'`` entry until
    the block ends, so a *new* negation nobody mirrored is picked up too — the
    test has to fail on an addition, not just on a removal.
    """
    out: list[str] = []
    in_code = False
    for raw in text.splitlines():
        line = raw.strip()
        if line == "code:":
            in_code = True
            continue
        if not in_code:
            continue
        if not line.startswith("- "):
            # The block ends at the next key (`predicate-quantifier:`) or any
            # non-item line; comments inside it are skipped rather than ending it.
            if line.startswith("#") or not line:
                continue
            break
        entry = line[2:].strip().strip("'\"")
        if entry.startswith("!"):
            out.append(entry[1:])
    return out


class CodeFilterParityTests(unittest.TestCase):
    """``CODE_FILTER_EXCLUDES`` must equal the workflow filter it mirrors.

    Silent drift is this mirror's designed failure mode, and its only symptom is
    a wasted 20-to-40-minute local Rust run — so the mirror is asserted against
    the source of truth rather than trusted to be maintained by hand.
    """

    def setUp(self):
        path = _repo_root() / rd.TESTS_WORKFLOW
        # FAIL, never skip. `TESTS_WORKFLOW` is the constant under test here: if
        # the workflow is renamed or moved, the pointer is stale AND the whole
        # parity class would silently skip — which is precisely the drift the
        # mirror exists to catch. (Contrast the optional-template skips
        # elsewhere, where the absent file is not itself the thing asserted.)
        self.assertTrue(
            path.is_file(),
            f"{rd.TESTS_WORKFLOW} not found from {_repo_root()} — the mirror's "
            "pointer is stale, so parity cannot be checked",
        )
        self.excludes = workflow_code_excludes(path.read_text(encoding="utf-8"))

    def test_the_parser_found_the_filter_at_all(self):
        # Guard the guard: a workflow reshuffle that broke the parser would
        # otherwise make this suite vacuously green.
        self.assertGreater(len(self.excludes), 10)
        self.assertIn("frontend/**", self.excludes)

    def test_the_mirror_matches_the_workflow_exactly(self):
        missing = [p for p in self.excludes if p not in rd.CODE_FILTER_EXCLUDES]
        extra = [p for p in rd.CODE_FILTER_EXCLUDES if p not in self.excludes]
        self.assertEqual(
            (missing, extra),
            ([], []),
            "CODE_FILTER_EXCLUDES has drifted from "
            f"{rd.TESTS_WORKFLOW}: missing {missing}, extra {extra}",
        )

    def test_the_frontend_workflow_is_mirrored(self):
        # The specific omission that cost a full local suite run on PR #333.
        self.assertIn(".github/workflows/frontend.yml", rd.CODE_FILTER_EXCLUDES)


class WorkflowExcludeParserTests(unittest.TestCase):
    """The hand parser itself, so the parity test rests on something tested."""

    def test_collects_only_negated_entries_of_the_code_block(self):
        text = "\n".join(
            [
                "        filters: |",
                "          code:",
                "          - '**'",
                "          - '!docs/**'",
                "          - '!**/*.md'",
                "          predicate-quantifier: 'every'",
                "          other:",
                "          - '!not-mine/**'",
            ]
        )
        self.assertEqual(workflow_code_excludes(text), ["docs/**", "**/*.md"])

    def test_a_comment_inside_the_block_does_not_end_it(self):
        text = "\n".join(
            [
                "          code:",
                "          - '**'",
                "          # a note",
                "          - '!cfg/**'",
            ]
        )
        self.assertEqual(workflow_code_excludes(text), ["cfg/**"])

    def test_no_code_block_yields_nothing(self):
        self.assertEqual(workflow_code_excludes("jobs:\n  build:\n"), [])


class MatchesTests(unittest.TestCase):
    """The three pattern shapes the path lists use."""

    def test_subtree_pattern(self):
        self.assertTrue(rd.matches("frontend/app/page.tsx", "frontend/**"))
        self.assertTrue(rd.matches("frontend", "frontend/**"))
        # A sibling sharing a string prefix must not match.
        self.assertFalse(rd.matches("frontend-old/app.tsx", "frontend/**"))
        self.assertFalse(rd.matches("programs/dropset/src/lib.rs", "frontend/**"))

    def test_suffix_pattern_matches_at_any_depth(self):
        self.assertTrue(rd.matches("README.md", "**/*.md"))
        self.assertTrue(rd.matches("docs/conventions/shell-commands.md", "**/*.md"))
        self.assertFalse(rd.matches("docs/notes.txt", "**/*.md"))

    def test_literal_pattern(self):
        self.assertTrue(rd.matches("pnpm-lock.yaml", "pnpm-lock.yaml"))
        self.assertFalse(rd.matches("sub/pnpm-lock.yaml", "pnpm-lock.yaml"))

    def test_matches_any(self):
        self.assertTrue(rd.matches_any("decks/app/x.tsx", rd.CODE_FILTER_EXCLUDES))
        self.assertFalse(
            rd.matches_any("programs/dropset/src/swap.rs", rd.CODE_FILTER_EXCLUDES)
        )

    def test_generation_inputs(self):
        self.assertTrue(
            rd.matches_any("programs/dropset/src/lib.rs", rd.GENERATION_INPUTS)
        )
        self.assertTrue(
            rd.matches_any("sdk/math-core/src/price.rs", rd.GENERATION_INPUTS)
        )
        self.assertFalse(rd.matches_any("tui/src/ui.rs", rd.GENERATION_INPUTS))
        # sdk/rs is a *consumer* of the generators, not an input to them.
        self.assertFalse(rd.matches_any("sdk/rs/src/events.rs", rd.GENERATION_INPUTS))

    def test_segment_anywhere_pattern(self):
        """``**/tests/**`` also ends in ``/**``, so it must be recognized as a
        segment-anywhere rule before the subtree rule swallows it."""
        self.assertTrue(rd.matches("sdk/rs/tests/parity.rs", "**/tests/**"))
        self.assertTrue(rd.matches("tests/x.rs", "**/tests/**"))
        self.assertFalse(rd.matches("sdk/rs/src/tests.rs", "**/tests/**"))
        # a segment that merely starts with the name must not match
        self.assertFalse(rd.matches("sdk/testsuite/x.rs", "**/tests/**"))


class SliceForTests(unittest.TestCase):
    """Which per-lens slice a changed path lands in."""

    def test_docs(self):
        self.assertEqual(rd.slice_for("docs/architecture.md"), "docs")
        self.assertEqual(rd.slice_for("README.md"), "docs")
        self.assertEqual(rd.slice_for("decks/demo-v1-spec.md"), "docs")

    def test_tests(self):
        self.assertEqual(rd.slice_for("sdk/rs/tests/parity.rs"), "tests")
        self.assertEqual(rd.slice_for(".claude/tools/tests/test_x.py"), "tests")
        self.assertEqual(rd.slice_for("frontend/app/x.test.tsx"), "tests")
        self.assertEqual(rd.slice_for("sdk/conformance/vectors.json"), "tests")

    def test_source_is_the_default(self):
        self.assertEqual(rd.slice_for("programs/dropset/src/swap.rs"), "source")
        self.assertEqual(rd.slice_for("tui/src/ui.rs"), "source")
        self.assertEqual(rd.slice_for("Cargo.toml"), "source")

    def test_markdown_under_a_tests_tree_reads_as_docs(self):
        """Docs is checked first, so a note beside the fixtures isn't handed to
        the completeness lens as if it were a test."""
        self.assertEqual(rd.slice_for("sdk/rs/tests/README.md"), "docs")

    def test_inline_rust_unit_tests_stay_in_source(self):
        """The split is by file, and Rust puts `#[cfg(test)]` in the source file —
        a documented limitation, asserted so it can't drift silently."""
        self.assertEqual(rd.slice_for("programs/dropset/src/swap.rs"), "source")

    def test_deck_code_is_source_and_only_deck_prose_is_docs(self):
        """`decks/**` as a whole tree put real application logic on the unsafe
        side of this module's own rationale: one diff of six `.tsx`/`.ts`/`.css`
        files produced `source: 0` / `docs: 224`, so the correctness, security
        and style lenses were each handed an empty file."""
        self.assertEqual(rd.slice_for("decks/app/DemoDeck.tsx"), "source")
        self.assertEqual(rd.slice_for("decks/app/page.ts"), "source")
        self.assertEqual(rd.slice_for("decks/styles/deck.css"), "source")
        # Deck prose still routes to docs.
        self.assertEqual(rd.slice_for("decks/demo-v1-spec.md"), "docs")


class CrateRollupTests(unittest.TestCase):
    """The tier decision's crate inputs. The multi-crate trigger weighed the
    PRESENCE of a second crate rather than what changed in it, so a seven-line
    doc fix in a second tree escalated a diff to the full fan-out."""

    @staticmethod
    def _files(*pairs):
        return [{"path": p, "changes": c} for p, c in pairs]

    def test_a_docs_only_second_crate_does_not_count_as_code(self):
        rollup = rd.crate_rollup(
            self._files(("sdk/rs/src/lib.rs", 40), ("docs/interface.md", 7))
        )
        self.assertTrue(rollup["sdk"]["has_source"])
        self.assertFalse(rollup["docs"]["has_source"])

    def test_two_source_crates_both_count(self):
        rollup = rd.crate_rollup(
            self._files(("sdk/rs/src/lib.rs", 4), ("programs/x/src/lib.rs", 9))
        )
        self.assertEqual(sum(1 for b in rollup.values() if b["has_source"]), 2)

    def test_changed_lines_are_attributed_per_slice(self):
        rollup = rd.crate_rollup(
            self._files(
                ("feeds/src/http.rs", 10),
                ("feeds/README.md", 3),
                ("feeds/tests/it.rs", 5),
            )
        )
        self.assertEqual(rollup["feeds"]["source"], 10)
        self.assertEqual(rollup["feeds"]["docs"], 3)
        self.assertEqual(rollup["feeds"]["tests"], 5)
        self.assertTrue(rollup["feeds"]["has_source"])

    def test_a_zero_change_entry_still_registers_the_crate(self):
        """A rename or mode change reports no line delta, and dropping it would
        under-count the tree it touched."""
        rollup = rd.crate_rollup(self._files(("sdk/rs/src/lib.rs", 0)))
        self.assertTrue(rollup["sdk"]["has_source"])

    def test_a_root_level_file_gets_its_own_bucket(self):
        rollup = rd.crate_rollup(self._files(("CLAUDE.md", 2)))
        self.assertIn("CLAUDE.md", rollup)
        self.assertFalse(rollup["CLAUDE.md"]["has_source"])

    def test_a_doc_comment_only_source_change_still_reads_as_source(self):
        """The stated bound: classification is by PATH, so this over-counts
        rather than under-reviewing. Pinned so the limitation is deliberate
        rather than discovered."""
        rollup = rd.crate_rollup(self._files(("sdk/rs/src/lib.rs", 7)))
        self.assertTrue(rollup["sdk"]["has_source"])


class DiffHeaderPathTests(unittest.TestCase):
    """Slicing keys on the destination path in each ``diff --git`` header."""

    def test_plain_header(self):
        self.assertEqual(
            rd._diff_header_path("diff --git a/src/x.rs b/src/x.rs\n"), "src/x.rs"
        )

    def test_rename_takes_the_destination(self):
        self.assertEqual(
            rd._diff_header_path("diff --git a/old/x.rs b/new/x.rs\n"), "new/x.rs"
        )

    def test_a_deletion_keeps_a_real_path_on_both_sides(self):
        """git puts /dev/null on the ---/+++ lines, never in this header, so there
        is no /dev/null case to handle."""
        self.assertEqual(
            rd._diff_header_path("diff --git a/gone.rs b/gone.rs\n"), "gone.rs"
        )

    def test_path_with_spaces(self):
        self.assertEqual(
            rd._diff_header_path("diff --git a/my dir/x.md b/my dir/x.md\n"),
            "my dir/x.md",
        )

    def test_non_header_is_none(self):
        self.assertIsNone(rd._diff_header_path("+added line\n"))
        self.assertIsNone(rd._diff_header_path("@@ -1,2 +1,3 @@\n"))


class GrepExcludesTests(unittest.TestCase):
    """The hoisted-grep exclude list, shared with the source-search wrapper."""

    def test_generated_dirs_reduce_to_their_basename(self):
        """grep --exclude-dir matches a basename, not a path — and one
        `generated` covers both the TS and Rust generated trees."""
        out = rd.grep_excludes()
        self.assertIn("generated", out["exclude_dirs"])

    def test_file_shaped_excludes_become_globs(self):
        out = rd.grep_excludes()
        self.assertIn("pnpm-lock.yaml", out["exclude_globs"])
        self.assertIn("Cargo.lock", out["exclude_globs"])
        self.assertIn("dropset.json", out["exclude_globs"])

    def test_never_search_dirs_are_included(self):
        """A grep exclude list that omits target/ is unusable here — it is
        multi-GB and gitignored, so `git diff` never needed to exclude it."""
        out = rd.grep_excludes()
        for name in ("target", "node_modules", ".git"):
            self.assertIn(name, out["exclude_dirs"])

    def test_tool_caches_are_excluded(self):
        """A cache stores content-addressed copies of the source, so a sweep hits
        every match twice without these."""
        out = rd.grep_excludes()
        for name in (".ruff_cache", ".pytest_cache", ".mypy_cache"):
            self.assertIn(name, out["exclude_dirs"])

    def test_grep_args_is_a_flag_string(self):
        out = rd.grep_excludes()
        self.assertIn("--exclude-dir=target", out["grep_args"])
        self.assertIn("--exclude=Cargo.lock", out["grep_args"])

    def test_no_duplicates(self):
        out = rd.grep_excludes()
        self.assertEqual(len(out["exclude_dirs"]), len(set(out["exclude_dirs"])))
        self.assertEqual(len(out["exclude_globs"]), len(set(out["exclude_globs"])))


def z(*records: str) -> str:
    """Join NUL-terminated ``--numstat -z`` fields into one payload.

    Every fixture goes through this rather than embedding ``\\0`` inline. A
    literal ``"…\\0"`` adjacent to a string starting with a digit is a trap: the
    formatter may merge the two, and ``\\0`` + ``1`` reads back as the *octal*
    escape ``\\01``, silently changing the fixture. ``\\x00`` here is
    unambiguous under any joining.
    """
    return "".join(f + "\x00" for f in records)


class ParseNumstatZTests(unittest.TestCase):
    """Fixtures here are **verbatim `git diff --numstat -z` output**, captured
    from a real repo rather than assumed. The earlier version of this suite
    asserted a rename shape git does not emit without `-z`, so it passed while
    the parser mishandled real renames."""

    def test_parses_and_sums_counts(self):
        text = z("3\t4\ttui/src/ui.rs", "10\t0\tdocs/a.md")
        self.assertEqual(
            rd.parse_numstat_z(text),
            [
                {"path": "docs/a.md", "changes": 10},
                {"path": "tui/src/ui.rs", "changes": 7},
            ],
        )

    def test_binary_file_lands_with_zero_changes(self):
        # git reports "-" for both counts on a binary file; it still changed.
        self.assertEqual(
            rd.parse_numstat_z(z("-\t-\tassets/logo.png")),
            [{"path": "assets/logo.png", "changes": 0}],
        )

    def test_rename_takes_the_destination_path(self):
        # Real -z rename layout: an EMPTY path field, then source, then dest.
        text = z("0\t0\t", "infra/aws/x.yaml", "cfg/x.yaml")
        self.assertEqual(
            rd.parse_numstat_z(text), [{"path": "cfg/x.yaml", "changes": 0}]
        )

    def test_rename_destination_is_gate_matchable(self):
        """The regression the old fixture hid: a cross-tree rename must yield a
        real path, not git's pretty `{old => new}` form, or it matches no prefix
        and the artifact gate silently fails OPEN."""
        text = z("0\t0\t", "sdk/math-core/gen.rs", "sdk/interface/gen.rs")
        files = rd.parse_numstat_z(text)
        self.assertEqual([f["path"] for f in files], ["sdk/interface/gen.rs"])
        self.assertTrue(rd.matches_any(files[0]["path"], rd.GENERATION_INPUTS))

    def test_rename_mixed_with_plain_entries(self):
        # The exact interleaving a real repo produced: rename, plain, binary,
        # non-ASCII (unquoted under -z), lockfile.
        text = z(
            "0\t0\t",
            "infra/aws/x.yaml",
            "cfg/x.yaml",
            "1\t0\tkeep.rs",
            "-\t-\tlogo.png",
            "1\t0\tnon-ascii dir/f.md",
            "4\t0\tpnpm-lock.yaml",
        )
        self.assertEqual(
            [f["path"] for f in rd.parse_numstat_z(text)],
            [
                "cfg/x.yaml",
                "keep.rs",
                "logo.png",
                "non-ascii dir/f.md",
                "pnpm-lock.yaml",
            ],
        )

    def test_non_ascii_path_is_not_quoted_under_z(self):
        # Plain --numstat emits this path quoted with octal escapes; -z passes
        # the bytes through verbatim, which is one reason -z is mandatory.
        got = rd.parse_numstat_z(z("1\t0\tcafé/f.md"))
        self.assertEqual(got, [{"path": "café/f.md", "changes": 1}])

    def test_ignores_short_records(self):
        self.assertEqual(rd.parse_numstat_z(""), [])
        self.assertEqual(rd.parse_numstat_z(z("not-a-row")), [])

    def test_truncated_rename_trailer_is_dropped_not_crashed(self):
        # A rename record whose path pair is cut off must not raise.
        self.assertEqual(rd.parse_numstat_z(z("0\t0\t", "only-source.rs")), [])


def git(repo: Path, *args):
    subprocess.run(
        ["git", *args],
        cwd=str(repo),
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


class GateTests(unittest.TestCase):
    """The verdict, over a real throwaway repo with an ``origin/main`` ref."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.out = self.root / "review-diff.txt"

        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.email", "t@example.com")
        git(self.repo, "config", "user.name", "T")
        git(self.repo, "config", "commit.gpgsign", "false")
        (self.repo / "seed.txt").write_text("seed\n", encoding="utf-8")
        git(self.repo, "add", "seed.txt")
        git(self.repo, "commit", "-q", "-m", "Seed")
        # A local ref standing in for the remote-tracking branch, so the tests
        # need no network and no second repository.
        git(self.repo, "update-ref", "refs/remotes/origin/main", "HEAD")

        self._cwd = Path.cwd()
        import os

        os.chdir(self.repo)

    def tearDown(self):
        import os

        os.chdir(self._cwd)
        self._tmp.cleanup()

    def commit(self, rel, text, message="Change"):
        target = self.repo / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")
        git(self.repo, "add", rel)
        git(self.repo, "commit", "-q", "-m", message)

    def test_clean_branch_is_ready(self):
        self.commit("programs/dropset/src/swap.rs", "fn swap() {}\n", "Add swap")
        v = rd.gate("main", self.out, fetch=False)
        self.assertTrue(v["ready"])
        self.assertTrue(v["base_fresh"])
        self.assertEqual(v["base_ahead"], [])
        self.assertEqual(v["blockers"], [])
        self.assertEqual(len(v["commits"]), 1)
        self.assertIn("Add swap", v["commits"][0])
        self.assertGreater(v["diff_lines"], 0)
        self.assertFalse(v["diff_empty"])
        self.assertEqual(
            [f["path"] for f in v["files"]], ["programs/dropset/src/swap.rs"]
        )
        self.assertTrue(self.out.exists())

    def test_nothing_changed_is_not_ready(self):
        v = rd.gate("main", self.out, fetch=False)
        self.assertFalse(v["ready"])
        self.assertTrue(v["diff_empty"])
        self.assertEqual(v["diff_lines"], 0)
        self.assertEqual(v["files"], [])
        self.assertTrue(any("no files changed" in b for b in v["blockers"]))

    def test_only_excluded_families_blocks_with_its_own_reason(self):
        """A lockfile-only PR is not "nothing to review, check the base" — it is
        "no source to review, but the regeneration gates still apply". The two
        used to collapse into one misleading blocker."""
        self.commit("pnpm-lock.yaml", "lockfileVersion: 9\n" * 20, "Bump deps")
        v = rd.gate("main", self.out, fetch=False)
        self.assertFalse(v["ready"])
        self.assertTrue(v["diff_empty"])
        self.assertEqual([f["path"] for f in v["files"]], ["pnpm-lock.yaml"])
        joined = " ".join(v["blockers"])
        self.assertIn("excluded generated family", joined)
        self.assertNotIn("no files changed", joined)
        # pnpm-lock.yaml is inside the CI code filter's exclusions.
        self.assertFalse(v["runs_rust_suites"])

    def test_ready_is_exactly_not_blockers(self):
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=False)
        self.assertEqual(v["ready"], not v["blockers"])

    def test_only_narrows_the_inventory_not_just_the_diff(self):
        # The inventory is what a caller reads to size and route the review, so
        # a `--only` run that still lists every changed path defeats the point
        # of scoping — and on a 58-file diff that is the whole result.
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        self.commit("docs/guide.md", "# Guide\n", "Docs")
        v = rd.gate("main", self.out, fetch=False, only=["tui/**"])
        self.assertEqual([f["path"] for f in v["files"]], ["tui/src/ui.rs"])
        self.assertTrue(v["ready"])

    def test_only_matches_a_single_segment_glob(self):
        # `dir/*.py` must not match `dir/sub/x.py`. Delegating to git's own
        # pathspec is what makes this hold without a second matcher to drift.
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        self.commit("tui/src/nested/deep.rs", "fn d() {}\n", "Nested")
        v = rd.gate("main", self.out, fetch=False, only=["tui/src/*.rs"])
        self.assertEqual([f["path"] for f in v["files"]], ["tui/src/ui.rs"])

    def test_an_only_that_matches_nothing_names_the_GLOB_not_the_branch(self):
        # The misleading case: with the inventory narrowed, an unmatched glob
        # makes `files` empty, which would otherwise report "no files changed
        # between the base and HEAD" and send the reader to check the branch.
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=False, only=["frontend/**"])
        self.assertFalse(v["ready"])
        joined = " ".join(v["blockers"])
        self.assertIn("matched none", joined)
        self.assertIn("frontend/**", joined)
        self.assertNotIn("no files changed", joined)

    def test_an_only_matching_solely_generated_families_says_so(self):
        self.commit("pnpm-lock.yaml", "lockfileVersion: 9\n" * 20, "Bump deps")
        v = rd.gate("main", self.out, fetch=False, only=["pnpm-lock.yaml"])
        self.assertFalse(v["ready"])
        joined = " ".join(v["blockers"])
        self.assertIn("excluded generated", joined)
        self.assertNotIn("matched none", joined)

    def test_failed_fetch_blocks_rather_than_claiming_freshness(self):
        """The tool must not report a base it could not verify as fresh. This
        repo has no `origin` remote, so a real fetch attempt fails."""
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=True)
        self.assertFalse(v["fetched"])
        self.assertIsNotNone(v["fetch_error"])
        self.assertFalse(v["ready"])
        self.assertTrue(any("could not fetch" in b for b in v["blockers"]))
        # The local ref still shows no divergence — which is precisely why
        # base_fresh alone must not be the gate.
        self.assertTrue(v["base_fresh"])

    def test_no_fetch_is_the_deliberate_override(self):
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=False)
        self.assertFalse(v["fetched"])
        self.assertIsNone(v["fetch_error"])
        self.assertTrue(v["ready"])

    def test_diff_file_is_owner_only(self):
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        rd.gate("main", self.out, fetch=False)
        self.assertEqual(self.out.stat().st_mode & 0o777, 0o600)

    def test_rejects_a_base_that_looks_like_an_option(self):
        with self.assertRaises(rd.ReviewDiffError):
            rd.gate("--upload-pack=evil", self.out, fetch=False)

    def test_cross_tree_rename_of_a_generation_input_still_trips_the_gate(self):
        """End-to-end guard for the parser fix: git reports this rename as
        `sdk/{math-core => interface}/gen.rs` without -z, which matches no
        GENERATION_INPUTS prefix and would skip the conformance gate."""
        self.commit("sdk/math-core/gen.rs", "fn gen() {}\n" * 30, "Add generator")
        (self.repo / "sdk" / "interface").mkdir(parents=True, exist_ok=True)
        git(self.repo, "mv", "sdk/math-core/gen.rs", "sdk/interface/gen.rs")
        git(self.repo, "commit", "-q", "-m", "Move generator across trees")
        v = rd.gate("main", self.out, fetch=False)
        paths = [f["path"] for f in v["files"]]
        self.assertNotIn("sdk/{math-core => interface}/gen.rs", paths)
        self.assertTrue(v["runs_artifact_gates"])

    def test_stale_base_is_not_ready(self):
        # Advance the branch, then advance origin/main *past* it — the phantom
        # deletion condition a line count cannot detect.
        self.commit("a.rs", "fn a() {}\n", "Branch work")
        git(self.repo, "checkout", "-q", "-b", "side", "HEAD~1")
        self.commit("landed.rs", "fn landed() {}\n", "Landed on main")
        git(self.repo, "update-ref", "refs/remotes/origin/main", "HEAD")
        git(self.repo, "checkout", "-q", "main")

        v = rd.gate("main", self.out, fetch=False)
        self.assertFalse(v["ready"])
        self.assertFalse(v["base_fresh"])
        self.assertEqual(len(v["base_ahead"]), 1)
        self.assertIn("Landed on main", v["base_ahead"][0])
        self.assertTrue(any("commit(s) HEAD lacks" in b for b in v["blockers"]))
        # The diff is non-empty, so line count alone would have said "fine".
        self.assertGreater(v["diff_lines"], 0)

    def test_generated_families_are_excluded_from_the_diff(self):
        self.commit("sdk/rs/src/generated/big.rs", "// generated\n" * 50, "Regenerate")
        v = rd.gate("main", self.out, fetch=False)
        self.assertEqual(v["diff_lines"], 0)
        # --numstat is unfiltered, so the file is still reported as changed.
        self.assertEqual(
            [f["path"] for f in v["files"]], ["sdk/rs/src/generated/big.rs"]
        )

    def test_docs_only_diff_skips_both_gate_families(self):
        self.commit("docs/conventions/x.md", "# x\n", "Doc tweak")
        v = rd.gate("main", self.out, fetch=False)
        self.assertTrue(v["ready"])
        self.assertFalse(v["runs_rust_suites"])
        self.assertFalse(v["runs_artifact_gates"])

    def test_program_diff_runs_both_gate_families(self):
        self.commit("programs/dropset/src/swap.rs", "fn swap() {}\n", "Program")
        v = rd.gate("main", self.out, fetch=False)
        self.assertTrue(v["runs_rust_suites"])
        self.assertTrue(v["runs_artifact_gates"])

    def test_tui_diff_runs_suites_but_not_artifact_gates(self):
        # tui/ is outside the CI code filter, but generates no artifact.
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=False)
        self.assertTrue(v["runs_rust_suites"])
        self.assertFalse(v["runs_artifact_gates"])

    def test_one_foreign_path_is_enough_to_run_the_suites(self):
        self.commit("docs/a.md", "# a\n", "Doc")
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=False)
        self.assertTrue(v["runs_rust_suites"])

    def test_cli_prints_json_and_signals_readiness_by_exit_code(self):
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = rd.run(
                [
                    "review_diff.py",
                    "--base",
                    "main",
                    "--out",
                    str(self.out),
                    "--no-fetch",
                ]
            )
        parsed = json.loads(buf.getvalue())
        self.assertEqual(code, 0)
        self.assertTrue(parsed["ready"])
        self.assertEqual(parsed["diff_path"], str(self.out))

    def test_cli_exits_non_zero_when_not_ready(self):
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = rd.run(
                [
                    "review_diff.py",
                    "--base",
                    "main",
                    "--out",
                    str(self.out),
                    "--no-fetch",
                ]
            )
        self.assertEqual(code, 1)
        self.assertFalse(json.loads(buf.getvalue())["ready"])

    def test_missing_base_ref_errors(self):
        with self.assertRaises(rd.ReviewDiffError):
            rd.gate("no-such-base", self.out, fetch=False)

    def test_split_partitions_the_diff_by_category(self):
        self.commit("programs/dropset/src/swap.rs", "fn swap() {}\n", "Src")
        self.commit("sdk/rs/tests/parity.rs", "fn parity() {}\n", "Tests")
        self.commit("docs/architecture.md", "# Arch\n", "Docs")
        v = rd.gate("main", self.out, fetch=False, split=True)

        slices = v["slices"]
        self.assertEqual(sorted(slices), ["docs", "source", "tests"])
        for name in ("source", "tests", "docs"):
            self.assertTrue(Path(slices[name]["path"]).exists())
            self.assertGreater(slices[name]["lines"], 0)

        source = Path(slices["source"]["path"]).read_text(encoding="utf-8")
        tests = Path(slices["tests"]["path"]).read_text(encoding="utf-8")
        docs = Path(slices["docs"]["path"]).read_text(encoding="utf-8")
        self.assertIn("swap.rs", source)
        self.assertNotIn("swap.rs", docs)
        self.assertNotIn("architecture.md", source)
        self.assertIn("parity.rs", tests)
        self.assertIn("architecture.md", docs)

    def test_split_line_counts_sum_to_the_whole_diff(self):
        """Every line lands in exactly one slice — a lens reading its slice can't
        be missing a hunk that the full diff had."""
        self.commit("programs/dropset/src/swap.rs", "fn swap() {}\n", "Src")
        self.commit("docs/architecture.md", "# Arch\n", "Docs")
        v = rd.gate("main", self.out, fetch=False, split=True)
        total = sum(s["lines"] for s in v["slices"].values())
        self.assertEqual(total, v["diff_lines"])

    def test_an_empty_category_is_still_written(self):
        """A missing file would be ambiguous between "no docs changed" and "the
        split didn't run"."""
        self.commit("programs/dropset/src/swap.rs", "fn swap() {}\n", "Src")
        v = rd.gate("main", self.out, fetch=False, split=True)
        docs = Path(v["slices"]["docs"]["path"])
        self.assertTrue(docs.exists())
        self.assertEqual(docs.read_text(encoding="utf-8"), "")
        self.assertEqual(v["slices"]["docs"]["lines"], 0)

    def test_slices_are_owner_only(self):
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=False, split=True)
        for s in v["slices"].values():
            self.assertEqual(Path(s["path"]).stat().st_mode & 0o777, 0o600)

    def test_no_slices_key_without_split(self):
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=False)
        self.assertNotIn("slices", v)

    def test_slice_paths_are_namespaced_off_the_out_stem(self):
        """Two --out paths must not share slice files.

        Fixed slice names made two runs in one scratchpad destroy each other's
        output silently: a scoped run with no docs hunks wrote an empty
        `review-diff-docs.txt` over an unscoped run's populated one, and the
        affected lens then correctly reported nothing to review. The stem
        derivation is what makes the skill's "hand each lens its own --out path"
        instruction actually true.
        """
        self.commit("docs/a.md", "# a\n", "Docs")
        first = rd.gate("main", self.out, fetch=False, split=True)
        other = self.out.parent / "review-diff-tools.txt"
        second = rd.gate("main", other, fetch=False, split=True)

        first_paths = {s["path"] for s in first["slices"].values()}
        second_paths = {s["path"] for s in second["slices"].values()}
        self.assertEqual(first_paths & second_paths, set())

    def test_the_default_out_keeps_its_documented_slice_names(self):
        """`--out review-diff.txt` must still yield review-diff-<name>.txt, which
        is what the skill prose and the lens-standing brief both name."""
        self.commit("docs/a.md", "# a\n", "Docs")
        v = rd.gate(
            "main", self.out.parent / "review-diff.txt", fetch=False, split=True
        )
        self.assertTrue(v["slices"]["docs"]["path"].endswith("review-diff-docs.txt"))

    def test_an_out_spelled_like_a_slice_is_still_refused(self):
        """The stem derivation stops slice-vs-slice clobber, not out-vs-slice.

        `--out review-diff-docs.txt` would write the WHOLE diff over a previous
        run's docs slice — the same silent overwrite by a different route — so
        that spelling stays refused deliberately rather than as a side effect of
        the old guard.
        """
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        for name in ("source", "tests", "docs"):
            with self.assertRaises(rd.ReviewDiffError):
                rd.gate(
                    "main",
                    self.out.parent / f"review-diff-{name}.txt",
                    fetch=False,
                    split=True,
                )

    def test_split_rewrites_slices_each_run(self):
        """Same rationale as the full diff: a stale slice is the hazard."""
        self.commit("docs/a.md", "# a\n", "Docs")
        v1 = rd.gate("main", self.out, fetch=False, split=True)
        docs_path = Path(v1["slices"]["docs"]["path"])
        self.assertIn("a.md", docs_path.read_text(encoding="utf-8"))

        git(self.repo, "update-ref", "refs/remotes/origin/main", "HEAD")
        self.commit("docs/b.md", "# b\n", "Docs 2")
        rd.gate("main", self.out, fetch=False, split=True)
        after = docs_path.read_text(encoding="utf-8")
        self.assertIn("b.md", after)
        self.assertNotIn("a.md", after)

    def test_cli_print_grep_excludes_needs_no_out(self):
        """It is a pure query about the exclude lists — no repo state, no git."""
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = rd.run(["review_diff.py", "--print-grep-excludes"])
        self.assertEqual(code, 0)
        parsed = json.loads(buf.getvalue())
        self.assertIn("generated", parsed["exclude_dirs"])
        self.assertIn("--exclude-dir=target", parsed["grep_args"])

    def test_cli_requires_out_otherwise(self):
        with self.assertRaises(rd.ReviewDiffError):
            rd.run(["review_diff.py", "--base", "main", "--no-fetch"])

    def test_cli_split_reports_slice_paths(self):
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = rd.run(
                [
                    "review_diff.py",
                    "--base",
                    "main",
                    "--out",
                    str(self.out),
                    "--no-fetch",
                    "--split",
                ]
            )
        self.assertEqual(code, 0)
        parsed = json.loads(buf.getvalue())
        self.assertIn("slices", parsed)
        self.assertTrue(Path(parsed["slices"]["source"]["path"]).exists())

    def test_gate_only_keeps_the_verdict_and_drops_the_inventory(self):
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = rd.run(
                [
                    "review_diff.py",
                    "--base",
                    "main",
                    "--out",
                    str(self.out),
                    "--no-fetch",
                    "--gate-only",
                ]
            )
        self.assertEqual(code, 0)
        parsed = json.loads(buf.getvalue())
        # The fields a mid-review re-check consumes.
        for field in ("base_fresh", "ready", "blockers", "base_ahead", "diff_empty"):
            self.assertIn(field, parsed)
        # The inventory, which is unbounded in the branch's size.
        for field in ("files", "commits", "diff_path", "diff_lines", "slices"):
            self.assertNotIn(field, parsed)

    def test_gate_only_still_gates_and_still_sets_the_exit_status(self):
        # The narrowing is a projection of the answer, not a cheaper way of
        # reaching one: an unready verdict must still exit non-zero.
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = rd.run(
                [
                    "review_diff.py",
                    "--base",
                    "main",
                    "--out",
                    str(self.out),
                    "--no-fetch",
                    "--gate-only",
                ]
            )
        self.assertEqual(code, 1)
        parsed = json.loads(buf.getvalue())
        self.assertFalse(parsed["ready"])
        self.assertTrue(parsed["blockers"])

    def test_gate_only_writes_the_diff_file_as_usual(self):
        # It narrows the printed JSON, not the work — the diff on disk is what
        # a later full call or a lens brief still points at.
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        with redirect_stdout(io.StringIO()):
            rd.run(
                [
                    "review_diff.py",
                    "--base",
                    "main",
                    "--out",
                    str(self.out),
                    "--no-fetch",
                    "--gate-only",
                ]
            )
        self.assertTrue(self.out.exists())


class GateOnlyProjectionTests(unittest.TestCase):
    """`gate_only` as a pure projection, independent of any repo."""

    def test_it_selects_exactly_the_named_fields(self):
        verdict = {k: k for k in rd.GATE_ONLY_FIELDS}
        verdict.update({"files": ["a"], "commits": ["b"], "slices": {}})
        self.assertEqual(set(rd.gate_only(verdict)), set(rd.GATE_ONLY_FIELDS))

    def test_a_missing_optional_field_is_skipped_not_faked(self):
        # `slices` is absent without --split, and `fetch_error` on the happy
        # path; projecting a `None` for either would read as a real value.
        self.assertEqual(rd.gate_only({"ready": True}), {"ready": True})

    def test_the_projection_never_invents_a_value(self):
        self.assertEqual(rd.gate_only({}), {})


class RustTestRangeTests(unittest.TestCase):
    """Finding the inline `#[cfg(test)]` regions a category split cannot see."""

    def test_a_simple_test_module_is_found(self):
        text = "fn a() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n"
        self.assertEqual(rd.rust_test_ranges(text), [(3, 6)])

    def test_nesting_inside_the_module_does_not_close_it_early(self):
        text = (
            "#[cfg(test)]\nmod tests {\n    fn t() {\n        if x { y(); }\n    }\n}\n"
        )
        self.assertEqual(rd.rust_test_ranges(text), [(1, 6)])

    def test_a_brace_inside_a_string_literal_does_not_close_the_module(self):
        text = '#[cfg(test)]\nmod tests {\n    let s = "}";\n    fn t() {}\n}\n'
        self.assertEqual(rd.rust_test_ranges(text), [(1, 5)])

    def test_a_brace_inside_a_raw_string_does_not_close_the_module(self):
        # Raw strings are common in test fixtures, which is exactly where this
        # parser runs — a false close would send real source into the tests slice.
        text = '#[cfg(test)]\nmod tests {\n    let s = r#"}"#;\n    fn t() {}\n}\n'
        self.assertEqual(rd.rust_test_ranges(text), [(1, 5)])

    def test_a_brace_in_a_line_comment_is_ignored(self):
        text = "#[cfg(test)]\nmod tests {\n    // }\n    fn t() {}\n}\n"
        self.assertEqual(rd.rust_test_ranges(text), [(1, 5)])

    def test_several_regions_are_all_found(self):
        text = "#[cfg(test)]\nmod a {\n}\nfn mid() {}\n#[cfg(test)]\nmod b {\n}\n"
        self.assertEqual(rd.rust_test_ranges(text), [(1, 3), (5, 7)])

    def test_a_file_with_no_tests_yields_nothing(self):
        self.assertEqual(rd.rust_test_ranges("fn a() {}\n"), [])

    def test_a_braceless_cfg_test_item_does_not_swallow_the_next_item(self):
        # `#[cfg(test)] mod test_support;` has no block at all. Walking to "the
        # next {" swallowed whatever braced item followed, routing PRODUCTION
        # code into the tests slice. The repo contains this exact idiom.
        text = (
            "#[cfg(test)]\n"
            "mod test_support;\n"
            "\n"
            "pub fn production() {\n"
            "    real_work();\n"
            "}\n"
        )
        ranges = rd.rust_test_ranges(text)
        self.assertEqual(ranges, [(1, 2)])
        self.assertFalse(rd._in_any_range(4, ranges))

    def test_a_braceless_cfg_test_use_does_not_swallow_the_next_item(self):
        text = "#[cfg(test)]\nuse std::sync::Arc;\n\npub fn production() {\n}\n"
        ranges = rd.rust_test_ranges(text)
        self.assertFalse(rd._in_any_range(4, ranges))

    def test_a_trailing_comment_does_not_defeat_the_braceless_terminator(self):
        # `mod test_support; // helpers only` does not END with `;`, so the
        # terminator missed it and the walk ran on to the next `{` — swallowing
        # production code into the tests slice, the very failure the braceless
        # case above exists to prevent.
        text = (
            "#[cfg(test)]\n"
            "mod test_support; // helpers only\n"
            "\n"
            "pub fn production() {\n"
            "    real_work();\n"
            "}\n"
        )
        ranges = rd.rust_test_ranges(text)
        self.assertEqual(ranges, [(1, 2)])
        self.assertFalse(rd._in_any_range(4, ranges))

    def test_a_lifetime_does_not_close_the_range_early(self):
        # `&'static str` is a lifetime, not a char literal. Treating it as an
        # open literal made the scan skip the trailing `{`, so the range ended
        # an item early and the rest of the module went back to `source`.
        text = (
            "#[cfg(test)]\n"
            "mod tests {\n"
            "    fn helper() -> &'static str {\n"
            '        "x"\n'
            "    }\n"
            "\n"
            "    #[test]\n"
            "    fn later() {}\n"
            "}\n"
            "pub fn production() {}\n"
        )
        ranges = rd.rust_test_ranges(text)
        self.assertEqual(ranges, [(1, 9)])
        self.assertTrue(rd._in_any_range(8, ranges))
        self.assertFalse(rd._in_any_range(10, ranges))

    def test_a_where_clause_lifetime_bound_is_also_not_a_literal(self):
        text = "#[cfg(test)]\nmod tests {\n    fn f<T>() where T: 'static {\n    }\n}\n"
        self.assertEqual(rd.rust_test_ranges(text), [(1, 5)])

    def test_a_real_char_literal_is_still_skipped(self):
        text = "#[cfg(test)]\nmod tests {\n    let c = '}';\n    fn t() {}\n}\n"
        self.assertEqual(rd.rust_test_ranges(text), [(1, 5)])

    def test_an_escaped_char_literal_is_still_skipped(self):
        text = "#[cfg(test)]\nmod tests {\n    let c = '\\'';\n    fn t() {}\n}\n"
        self.assertEqual(rd.rust_test_ranges(text), [(1, 5)])

    def test_an_unbalanced_region_claims_to_end_of_file_rather_than_vanishing(self):
        # Losing a test region to the source slice is the failure being fixed,
        # so an unparsable tail errs toward `tests`.
        text = "#[cfg(test)]\nmod tests {\n    fn t() {}\n"
        self.assertEqual(rd.rust_test_ranges(text), [(1, 3)])


class InlineRustTestSplitTests(unittest.TestCase):
    """The end-to-end property: a Rust diff's test hunks reach the tests slice.

    The regression: a 4,351-line Rust diff produced a **zero-line** tests slice
    because inline `#[cfg(test)]` rides the source file, so the test-adequacy
    lens read three source slices and became the run's costliest agent.
    """

    SOURCE = (
        "pub fn add(a: i32, b: i32) -> i32 {\n"
        "    a + b\n"
        "}\n"
        "\n"
        "#[cfg(test)]\n"
        "mod tests {\n"
        "    use super::*;\n"
        "\n"
        "    #[test]\n"
        "    fn it_adds() {\n"
        "        assert_eq!(add(1, 2), 3);\n"
        "    }\n"
        "}\n"
    )

    DIFF = (
        "diff --git a/src/lib.rs b/src/lib.rs\n"
        "index 111..222 100644\n"
        "--- a/src/lib.rs\n"
        "+++ b/src/lib.rs\n"
        "@@ -1,3 +1,3 @@\n"
        " pub fn add(a: i32, b: i32) -> i32 {\n"
        "-    a - b\n"
        "+    a + b\n"
        " }\n"
        "@@ -9,3 +9,3 @@\n"
        "     #[test]\n"
        "-    fn it_adds() { assert_eq!(add(1, 2), 4); }\n"
        "+    fn it_adds() { assert_eq!(add(1, 2), 3); }\n"
        "\n"
    )

    def _split(self):
        d = Path(self.tmp.name)
        (d / "src").mkdir(parents=True, exist_ok=True)
        (d / "src" / "lib.rs").write_text(self.SOURCE, encoding="utf-8")
        diff_path = d / "review-diff.txt"
        diff_path.write_text(self.DIFF, encoding="utf-8")
        cwd = os.getcwd()
        try:
            os.chdir(d)
            return rd.split_diff(diff_path, d)
        finally:
            os.chdir(cwd)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def test_the_test_hunk_lands_in_the_tests_slice(self):
        slices = self._split()
        tests_text = Path(slices["tests"]["path"]).read_text(encoding="utf-8")
        self.assertIn("it_adds", tests_text)
        self.assertGreater(slices["tests"]["lines"], 0)

    def test_the_source_hunk_stays_in_the_source_slice(self):
        slices = self._split()
        source_text = Path(slices["source"]["path"]).read_text(encoding="utf-8")
        self.assertIn("a + b", source_text)
        self.assertNotIn("it_adds", source_text)

    def test_each_slice_carries_the_file_header_so_it_reads_as_a_diff(self):
        # A hunk without its `diff --git`/`+++` preamble is not readable as a
        # diff, and a lens is handed the slice file directly.
        slices = self._split()
        for name in ("source", "tests"):
            text = Path(slices[name]["path"]).read_text(encoding="utf-8")
            self.assertIn("diff --git a/src/lib.rs b/src/lib.rs", text)
            self.assertIn("+++ b/src/lib.rs", text)

    def test_an_ADDED_rust_file_carries_a_complete_header_into_each_slice(self):
        # `new file mode` is not `index `/`--- `/`+++ `, so a prefix whitelist
        # let it through early and the tests slice got `diff --git` immediately
        # followed by `@@` — no ---/+++ at all, i.e. a malformed diff handed to
        # a lens.
        d = Path(self.tmp.name)
        (d / "src").mkdir(parents=True, exist_ok=True)
        (d / "src" / "added.rs").write_text(self.SOURCE, encoding="utf-8")
        diff_path = d / "review-diff.txt"
        diff_path.write_text(
            "diff --git a/src/added.rs b/src/added.rs\n"
            "new file mode 100644\n"
            "index 0000000..111\n"
            "--- /dev/null\n"
            "+++ b/src/added.rs\n"
            "@@ -0,0 +1,3 @@\n"
            "+pub fn add(a: i32, b: i32) -> i32 {\n"
            "+    a + b\n"
            "+}\n"
            "@@ -0,0 +9,3 @@\n"
            "+    #[test]\n"
            "+    fn it_adds() {}\n"
            "+\n",
            encoding="utf-8",
        )
        cwd = os.getcwd()
        try:
            os.chdir(d)
            slices = rd.split_diff(diff_path, d)
        finally:
            os.chdir(cwd)
        for name in ("source", "tests"):
            text = Path(slices[name]["path"]).read_text(encoding="utf-8")
            self.assertIn("diff --git a/src/added.rs", text)
            self.assertIn("new file mode 100644", text)
            self.assertIn("+++ b/src/added.rs", text)

    def test_an_all_tests_file_leaves_the_source_slice_empty(self):
        # A body line belongs to ITS OWN hunk's slice. Flushing the header to
        # `source` on every body line instead wrote the whole preamble into the
        # source slice for a file with no source hunks at all — a phantom "file
        # changed" entry with nothing after it, and a non-zero source count,
        # which is exactly the signal a caller reads as "spawn a source lens".
        d = Path(self.tmp.name)
        (d / "src").mkdir(parents=True, exist_ok=True)
        (d / "src" / "lib.rs").write_text(self.SOURCE, encoding="utf-8")
        diff_path = d / "review-diff.txt"
        diff_path.write_text(
            "diff --git a/src/lib.rs b/src/lib.rs\n"
            "index 111..222 100644\n"
            "--- a/src/lib.rs\n"
            "+++ b/src/lib.rs\n"
            "@@ -9,3 +9,3 @@ mod tests {\n"
            "-    fn it_adds() { assert_eq!(add(1, 2), 4); }\n"
            "+    fn it_adds() { assert_eq!(add(1, 2), 3); }\n"
            "\n",
            encoding="utf-8",
        )
        cwd = os.getcwd()
        try:
            os.chdir(d)
            slices = rd.split_diff(diff_path, d)
        finally:
            os.chdir(cwd)
        self.assertEqual(slices["source"]["lines"], 0)
        source_text = Path(slices["source"]["path"]).read_text(encoding="utf-8")
        self.assertNotIn("diff --git", source_text)
        self.assertIn("it_adds", Path(slices["tests"]["path"]).read_text("utf-8"))

    def test_a_header_only_diff_is_not_dropped_from_every_slice(self):
        # A mode change has no hunk at all, so nothing ever triggered a flush
        # and the file vanished from the output entirely.
        d = Path(self.tmp.name)
        (d / "src").mkdir(parents=True, exist_ok=True)
        (d / "src" / "lib.rs").write_text(self.SOURCE, encoding="utf-8")
        diff_path = d / "review-diff.txt"
        diff_path.write_text(
            "diff --git a/src/lib.rs b/src/lib.rs\nold mode 100644\nnew mode 100755\n",
            encoding="utf-8",
        )
        cwd = os.getcwd()
        try:
            os.chdir(d)
            slices = rd.split_diff(diff_path, d)
        finally:
            os.chdir(cwd)
        joined = "".join(
            Path(slices[name]["path"]).read_text(encoding="utf-8")
            for name in ("source", "tests", "docs")
        )
        self.assertIn("diff --git a/src/lib.rs", joined)
        self.assertIn("new mode 100755", joined)

    def test_a_rust_file_with_no_inline_tests_is_untouched(self):
        d = Path(self.tmp.name)
        (d / "src").mkdir(parents=True, exist_ok=True)
        (d / "src" / "plain.rs").write_text("fn a() {}\n", encoding="utf-8")
        diff_path = d / "review-diff.txt"
        diff_path.write_text(
            "diff --git a/src/plain.rs b/src/plain.rs\n"
            "--- a/src/plain.rs\n"
            "+++ b/src/plain.rs\n"
            "@@ -1 +1 @@\n"
            "-fn a() {}\n"
            "+fn a() {}\n",
            encoding="utf-8",
        )
        cwd = os.getcwd()
        try:
            os.chdir(d)
            slices = rd.split_diff(diff_path, d)
        finally:
            os.chdir(cwd)
        self.assertEqual(slices["tests"]["lines"], 0)
        self.assertGreater(slices["source"]["lines"], 0)


class OverlappingPrsTests(unittest.TestCase):
    """In-flight overlap: reported, never blocking, and never in context."""

    ROWS = [
        {
            "number": 12,
            "title": "Other work",
            "headRefName": "eng-12",
            "files": [{"path": "src/lib.rs"}, {"path": "README.md"}],
        },
        {
            "number": 13,
            "title": "Unrelated",
            "headRefName": "eng-13",
            "files": [{"path": "docs/other.md"}],
        },
    ]

    def _run(self, paths, rows=None, branch="eng-99"):
        completed = subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout=json.dumps(self.ROWS if rows is None else rows),
        )
        with (
            mock.patch.object(rd.subprocess, "run", return_value=completed),
            mock.patch.object(rd, "_git", return_value=branch),
        ):
            return rd.overlapping_prs(paths)

    def test_an_intersecting_pr_is_reported_with_its_shared_files(self):
        result = self._run(["src/lib.rs"])
        self.assertTrue(result["checked"])
        self.assertEqual([p["number"] for p in result["prs"]], [12])
        self.assertEqual(result["prs"][0]["shared_files"], ["src/lib.rs"])

    def test_a_non_intersecting_pr_is_not_reported(self):
        result = self._run(["src/only_mine.rs"])
        self.assertEqual(result["prs"], [])

    def test_the_pr_for_this_branch_is_excluded(self):
        result = self._run(["src/lib.rs"], branch="eng-12")
        self.assertEqual(result["prs"], [])

    def test_results_are_ranked_by_how_much_they_overlap(self):
        result = self._run(["src/lib.rs", "README.md", "docs/other.md"])
        self.assertEqual([p["number"] for p in result["prs"]], [12, 13])

    def test_a_missing_gh_is_reported_not_raised(self):
        # Overlap is advisory, so an unavailable gh must not fail the gate.
        with mock.patch.object(rd.subprocess, "run", side_effect=OSError("no gh")):
            result = rd.overlapping_prs(["src/lib.rs"])
        self.assertFalse(result["checked"])
        self.assertIn("no gh", result["error"])
        self.assertEqual(result["prs"], [])

    def test_a_gh_failure_is_reported_not_raised(self):
        completed = subprocess.CompletedProcess(
            args=[], returncode=1, stdout="", stderr="not authenticated"
        )
        with mock.patch.object(rd.subprocess, "run", return_value=completed):
            result = rd.overlapping_prs(["src/lib.rs"])
        self.assertFalse(result["checked"])
        self.assertIn("not authenticated", result["error"])


if __name__ == "__main__":
    unittest.main()
