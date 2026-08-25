#!/usr/bin/env python3
"""Unit tests for ``docker_context.py`` (stdlib ``unittest``; no pytest).

Two kinds of test here, and the second kind is the one that matters most.

The **unit** tests pin the pattern matcher, because the whole guard rests on it
and Docker's semantics are not gitignore's: ``*`` must not cross a separator,
``**/x`` must match ``x`` at depth zero as well as nested, a trailing slash is
dropped rather than meaning directory-only, and the *last* matching rule wins
so a ``!`` re-include works regardless of order.

The **repo** tests (``TestRealIgnoreFile``) assert against this checkout's own
``.dockerignore``. They exist because the expensive failure is not a bad regex
— it is an ignore file that silently starves the build. ``cargo`` needs every
workspace member's manifest to resolve the workspace at all, so one
over-broad pattern turns every image build into a confusing resolver error.
The member list is **parsed from Cargo.toml at run time** rather than
hardcoded, so adding a crate extends this test automatically instead of
leaving it to pass on a stale list.
"""

from __future__ import annotations

import io
import os
import re
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout

import docker_context as dc

# Pulls the quoted entries out of the workspace `members = [...]` block.
_MEMBERS_RE = re.compile(r"members\s*=\s*\[(.*?)\]", re.DOTALL)


def ignored(pattern: str, path: str) -> bool:
    """Whether a single ``pattern`` excludes ``path``."""
    return dc.is_ignored(path, dc.parse_dockerignore(pattern))


def write(root: str, rel: str, body: str = "x") -> str:
    """Create a file (and its parents) under ``root``, returning its path."""
    full = os.path.join(root, rel)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w", encoding="utf-8") as handle:
        handle.write(body)
    return full


class TestCleanPattern(unittest.TestCase):
    def test_strips_trailing_separator(self) -> None:
        # Docker has no directory-only form, so `target/` and `target` are the
        # same pattern. Getting this wrong would make the required-pattern
        # check reject a legitimate spelling.
        self.assertEqual(dc.clean_pattern("target/"), "target")
        self.assertEqual(dc.clean_pattern("**/target/"), "**/target")

    def test_strips_leading_dot_slash(self) -> None:
        self.assertEqual(dc.clean_pattern("./target"), "target")
        self.assertEqual(dc.clean_pattern("././target"), "target")

    def test_strips_surrounding_whitespace(self) -> None:
        self.assertEqual(dc.clean_pattern("  target  "), "target")


class TestParse(unittest.TestCase):
    def test_drops_comments_and_blanks(self) -> None:
        rules = dc.parse_dockerignore("# a comment\n\n  \ntarget\n")
        self.assertEqual([rule.pattern for rule in rules], ["target"])

    def test_marks_negation(self) -> None:
        rules = dc.parse_dockerignore("target\n!target/keep\n")
        self.assertEqual([rule.negated for rule in rules], [False, True])
        self.assertEqual(rules[1].pattern, "target/keep")

    def test_bare_bang_is_not_a_rule(self) -> None:
        # `!` alone cleans to the empty pattern; emitting a rule for it would
        # match everything and un-ignore the entire context.
        self.assertEqual(dc.parse_dockerignore("!\n"), [])


class TestMatching(unittest.TestCase):
    def test_double_star_matches_at_any_depth(self) -> None:
        for path in ("target", "bots/maker-bot/target", "a/b/c/target"):
            with self.subTest(path=path):
                self.assertTrue(ignored("**/target", path))

    def test_double_star_does_not_match_a_prefix(self) -> None:
        # The 57 GB worktree tree is caught by an anchored pattern, so a
        # matcher that treated `**/target` as a substring search would look
        # like it worked while quietly excluding real crates.
        for path in ("targets", "my-target", "target-dir/x"):
            with self.subTest(path=path):
                self.assertFalse(ignored("**/target", path))

    def test_single_star_does_not_cross_a_separator(self) -> None:
        # Probed with a pattern whose ancestors cannot match, so this isolates
        # the matcher. Note `sdk/*` would legitimately exclude
        # `sdk/rs/Cargo.toml` — not by crossing the separator, but because it
        # matches the directory `sdk/rs`, whose contents then go with it.
        self.assertTrue(ignored("sdk/*.toml", "sdk/Cargo.toml"))
        self.assertFalse(ignored("sdk/*.toml", "sdk/rs/Cargo.toml"))
        self.assertFalse(ignored("*.toml", "sdk/Cargo.toml"))

    def test_question_mark_matches_one_non_separator(self) -> None:
        self.assertTrue(ignored("a?c", "abc"))
        self.assertFalse(ignored("a?c", "a/c"))

    def test_anchored_pattern_does_not_match_elsewhere(self) -> None:
        self.assertTrue(ignored("frontend/public", "frontend/public"))
        self.assertFalse(ignored("frontend/public", "decks/public"))

    def test_directory_pattern_matches_the_directory_itself(self) -> None:
        # `measure` prunes on the directory's own path, so this is what makes
        # a whole subtree drop out.
        self.assertTrue(ignored(".claude/worktrees", ".claude/worktrees"))

    def test_last_match_wins(self) -> None:
        rules = dc.parse_dockerignore("**/target\n!keep/target\n")
        self.assertTrue(dc.is_ignored("a/target", rules))
        self.assertFalse(dc.is_ignored("keep/target", rules))

    def test_negation_order_does_not_matter(self) -> None:
        # Docker evaluates every rule and takes the final match, so a negation
        # written before the rule it overrides still loses to it.
        rules = dc.parse_dockerignore("!keep/target\n**/target\n")
        self.assertTrue(dc.is_ignored("keep/target", rules))

    def test_an_excluded_directory_covers_its_descendants(self) -> None:
        # Excluding a directory excludes its contents, so a pattern naming
        # only the directory must still answer for a file deep inside it.
        # Matching the full path alone reported this as included, which made
        # the function disagree with both Docker and `measure`'s pruning.
        rules = dc.parse_dockerignore("frontend/public\n")
        self.assertTrue(dc.is_ignored("frontend/public", rules))
        self.assertTrue(dc.is_ignored("frontend/public/flags/us.svg", rules))
        self.assertFalse(dc.is_ignored("frontend/lib/x.ts", rules))

    def test_a_negation_can_re_include_at_depth(self) -> None:
        # The ancestor walk inherits rather than short-circuits, so a deeper
        # re-include still gets its say after its parent was excluded.
        rules = dc.parse_dockerignore("frontend/public\n!frontend/public/keep\n")
        self.assertTrue(dc.is_ignored("frontend/public/other", rules))
        self.assertFalse(dc.is_ignored("frontend/public/keep", rules))

    def test_dotted_pattern_is_literal(self) -> None:
        self.assertTrue(ignored("**/.env.*", "frontend/.env.local"))
        self.assertFalse(ignored("**/.env.*", "frontend/env-local"))


class TestMeasure(unittest.TestCase):
    def test_sums_only_unignored_files(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            write(root, "Cargo.toml", "abcde")
            write(root, "target/big.o", "0" * 5000)
            result = dc.measure(root, dc.parse_dockerignore("**/target\n"))
            self.assertEqual(result.files, 1)
            self.assertEqual(result.total_bytes, 5)

    def test_prunes_rather_than_descends(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            write(root, "node_modules/a/b/c/deep.js")
            write(root, "keep.rs")
            result = dc.measure(root, dc.parse_dockerignore("**/node_modules\n"))
            self.assertEqual(result.files, 1)
            self.assertEqual([path for path, _ in result.pruned], ["node_modules"])

    def test_no_rules_measures_everything(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            write(root, "a.rs", "xx")
            write(root, "target/b.o", "yyy")
            result = dc.measure(root, [])
            self.assertEqual(result.files, 2)
            self.assertEqual(result.total_bytes, 5)

    def test_dangling_symlink_does_not_abort_the_walk(self) -> None:
        # A worktree's operator-file symlink points into the base checkout; if
        # that moves, the link dangles. Measuring must still finish, since the
        # guard runs in exactly that worktree.
        with tempfile.TemporaryDirectory() as root:
            write(root, "keep.rs", "abc")
            os.symlink(
                os.path.join(root, "nowhere", "gone.env"),
                os.path.join(root, "dangling.env"),
            )
            result = dc.measure(root, [])
            self.assertIn("keep.rs", os.listdir(root))
            self.assertGreaterEqual(result.files, 1)

    def test_symlink_counts_itself_not_its_target(self) -> None:
        # lstat, not stat: a link to a 1 MB file must not be billed as 1 MB,
        # or a worktree's env symlinks would inflate the measurement.
        with tempfile.TemporaryDirectory() as root:
            write(root, "real.bin", "z" * 100000)
            os.symlink(os.path.join(root, "real.bin"), os.path.join(root, "link.bin"))
            result = dc.measure(root, dc.parse_dockerignore("real.bin\n"))
            self.assertEqual(result.files, 1)
            self.assertLess(result.total_bytes, 1000)


class TestMissingPatterns(unittest.TestCase):
    def test_all_present_reports_nothing(self) -> None:
        text = "\n".join(dc.REQUIRED_PATTERNS)
        self.assertEqual(dc.missing_patterns(text), [])

    def test_reports_what_is_absent(self) -> None:
        text = "\n".join(list(dc.REQUIRED_PATTERNS)[:-1])
        missing = dc.missing_patterns(text)
        self.assertEqual(missing, [list(dc.REQUIRED_PATTERNS)[-1]])

    def test_accepts_an_equivalent_spelling(self) -> None:
        # `**/target/` cleans to `**/target`, so a trailing slash must not read
        # as a missing pattern.
        text = "\n".join(f"{name}/" for name in dc.REQUIRED_PATTERNS)
        self.assertEqual(dc.missing_patterns(text), [])

    def test_a_negated_line_does_not_count_as_present(self) -> None:
        # `!**/target` re-includes the tree; counting it as the required
        # pattern would let the guard pass on a file that does the opposite.
        text = "\n".join(dc.REQUIRED_PATTERNS).replace("**/target", "!**/target")
        self.assertIn("**/target", dc.missing_patterns(text))


class TestCheck(unittest.TestCase):
    def test_missing_file_fails_and_lists_requirements(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            code, lines = dc.check(root)
            self.assertEqual(code, 1)
            body = "\n".join(lines)
            self.assertIn("no .dockerignore", body)
            for name in dc.REQUIRED_PATTERNS:
                self.assertIn(name, body)

    def test_missing_pattern_fails_and_names_it(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            kept = [n for n in dc.REQUIRED_PATTERNS if n != "**/.next"]
            write(root, ".dockerignore", "\n".join(kept))
            code, lines = dc.check(root)
            self.assertEqual(code, 1)
            self.assertIn("**/.next", "\n".join(lines))

    def test_complete_file_passes(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            write(root, ".dockerignore", "\n".join(dc.REQUIRED_PATTERNS))
            write(root, "Cargo.toml", "x")
            code, lines = dc.check(root)
            self.assertEqual(code, 0, "\n".join(lines))
            self.assertIn("ok", "\n".join(lines))

    def test_ceiling_trips_on_a_tree_with_no_pattern(self) -> None:
        # The case the presence check cannot catch: a new fat tree nobody
        # wrote a pattern for.
        with tempfile.TemporaryDirectory() as root:
            write(root, ".dockerignore", "\n".join(dc.REQUIRED_PATTERNS))
            write(root, "surprise/blob.bin", "0" * 4096)
            original = dc.CEILING_BYTES
            dc.CEILING_BYTES = 1024
            try:
                code, lines = dc.check(root)
            finally:
                dc.CEILING_BYTES = original
            self.assertEqual(code, 1)
            self.assertIn("ceiling", "\n".join(lines))


class TestFindRoot(unittest.TestCase):
    def test_walks_up_to_the_markers(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            write(root, "Cargo.toml")
            write(root, "pnpm-workspace.yaml")
            deep = os.path.join(root, "a", "b")
            os.makedirs(deep, exist_ok=True)
            self.assertEqual(
                os.path.realpath(dc.find_root(deep)), os.path.realpath(root)
            )

    def test_raises_when_there_is_no_root(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            with self.assertRaises(SystemExit):
                dc.find_root(root)


class TestHuman(unittest.TestCase):
    def test_scales_to_the_largest_unit(self) -> None:
        self.assertEqual(dc.human(512), "512 B")
        self.assertEqual(dc.human(2048), "2.0 KB")
        self.assertEqual(dc.human(5 * 1024 * 1024), "5.0 MB")
        self.assertEqual(dc.human(3 * 1024**3), "3.0 GB")


class TestMain(unittest.TestCase):
    def test_measure_prints_a_size(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            write(root, "a.rs", "abc")
            buffer = io.StringIO()
            with redirect_stdout(buffer):
                code = dc.main(["--measure", "--root", root])
            self.assertEqual(code, 0)
            self.assertIn("context:", buffer.getvalue())

    def test_measure_no_ignore_skips_the_file(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            write(root, ".dockerignore", "**/target")
            write(root, "target/big.o", "0" * 4096)
            plain = io.StringIO()
            with redirect_stdout(plain):
                dc.main(["--measure", "--root", root])
            baseline = io.StringIO()
            with redirect_stdout(baseline):
                dc.main(["--measure", "--root", root, "--no-ignore"])
            # The ignore file is itself part of the context — nothing excludes
            # it — so the honest comparison is that the 4 KB blob drops out,
            # not that the count reaches zero.
            self.assertIn("pruned", plain.getvalue())
            self.assertIn("target", plain.getvalue())
            self.assertIn("4.0 KB", baseline.getvalue())
            self.assertNotIn("KB", plain.getvalue().split("pruned")[0])

    def test_check_failure_goes_to_stderr(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            buffer = io.StringIO()
            with redirect_stderr(buffer):
                code = dc.main(["--root", root])
            self.assertEqual(code, 1)
            self.assertIn("no .dockerignore", buffer.getvalue())


class TestRealIgnoreFile(unittest.TestCase):
    """Assert against this checkout's own ``.dockerignore``.

    The unit tests above prove the matcher behaves; these prove the file we
    ship is correct. Over-exclusion is the dangerous direction — cargo cannot
    resolve a workspace with a member manifest missing — so most of these
    assert what must **survive**.
    """

    @classmethod
    def setUpClass(cls) -> None:
        cls.root = dc.find_root(os.path.dirname(os.path.abspath(dc.__file__)))
        with open(os.path.join(cls.root, ".dockerignore"), encoding="utf-8") as handle:
            cls.rules = dc.parse_dockerignore(handle.read())

    def members(self) -> list[str]:
        """The workspace members, read from Cargo.toml at run time."""
        with open(os.path.join(self.root, "Cargo.toml"), encoding="utf-8") as handle:
            block = _MEMBERS_RE.search(handle.read())
        self.assertIsNotNone(block, "no members = [...] in Cargo.toml")
        assert block is not None
        found = re.findall(r'"([^"]+)"', block.group(1))
        self.assertTrue(found, "parsed an empty workspace member list")
        return found

    def test_the_guard_passes_on_this_checkout(self) -> None:
        code, lines = dc.check(self.root)
        self.assertEqual(code, 0, "\n".join(lines))

    def test_workspace_manifests_survive(self) -> None:
        # cargo needs every member's manifest to resolve the workspace, so a
        # single over-broad pattern here breaks all four Rust image builds.
        for member in self.members():
            manifest = f"{member}/Cargo.toml"
            with self.subTest(path=manifest):
                self.assertFalse(dc.is_ignored(manifest, self.rules))
                self.assertTrue(
                    os.path.exists(os.path.join(self.root, manifest)),
                    f"{manifest} is in Cargo.toml but not on disk",
                )

    def test_member_sources_survive(self) -> None:
        for member in self.members():
            source = f"{member}/src/lib.rs"
            with self.subTest(path=source):
                self.assertFalse(dc.is_ignored(source, self.rules))

    def test_root_build_inputs_survive(self) -> None:
        for path in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
            with self.subTest(path=path):
                self.assertFalse(dc.is_ignored(path, self.rules))
                self.assertTrue(os.path.exists(os.path.join(self.root, path)))

    def test_the_asm_tree_survives(self) -> None:
        # programs/dropset/build.rs expands src/asm into OUT_DIR on every
        # build, so the assembly has to be in the context.
        self.assertFalse(
            dc.is_ignored("programs/dropset/src/asm/entrypoint.s", self.rules)
        )

    def test_migrations_survive(self) -> None:
        # The migrate image embeds the migration set; excluding it would make
        # a schema step exit clean having applied nothing.
        self.assertFalse(
            dc.is_ignored("db-schema/migrations/0001_init.sql", self.rules)
        )

    def test_the_fat_trees_are_excluded(self) -> None:
        for path in (
            "target",
            "bots/maker-bot/target",
            ".claude/worktrees",
            ".claude/worktrees/eng-1/target",
            ".git",
            "node_modules",
            "frontend/node_modules",
            "frontend/.next",
            "frontend/public/flags/us.svg",
            "decks",
            "brand-assets",
        ):
            with self.subTest(path=path):
                self.assertTrue(dc.is_ignored(path, self.rules))

    def test_env_files_are_excluded(self) -> None:
        # An env file baked into a layer stays in the image.
        for path in (
            "frontend/.env.local",
            "infra/localnet/secrets.local.env",
            ".env",
        ):
            with self.subTest(path=path):
                self.assertTrue(dc.is_ignored(path, self.rules))


if __name__ == "__main__":
    unittest.main()
