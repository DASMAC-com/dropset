"""Stdlib ``unittest`` tests for prune_conversations' pure decision logic —
the age/open-PR rule, worktree parsing, the dropset-set derivation, and the
under-root path guard. The filesystem scan/delete driver is exercised through
these units; run with ``python3 -m unittest`` from ``.claude/tools``.
"""

import tempfile
import unittest
from pathlib import Path

from prune_conversations import (
    decide_history,
    decide_slug,
    dropset_slug_sets,
    is_within,
    parse_worktrees,
    slugify,
)

# A fixed "now" so age comparisons are deterministic; cutoff = now - 2 days.
NOW = 1_000_000.0
DAY = 86_400.0
CUTOFF = NOW - 2 * DAY  # entries with mtime < CUTOFF are "aged"
OLD = CUTOFF - 10_000  # comfortably older than the threshold
FRESH = CUTOFF + 10_000  # comfortably within the threshold


class SlugifyTests(unittest.TestCase):
    def test_replaces_slashes_and_dots(self):
        self.assertEqual(
            slugify(Path("/Users/alex/repos/dropset")), "-Users-alex-repos-dropset"
        )
        self.assertEqual(
            slugify(Path("/a/.claude/worktrees/eng-663")),
            "-a--claude-worktrees-eng-663",
        )


class ParseWorktreesTests(unittest.TestCase):
    def test_parses_paths_and_short_branches(self):
        porcelain = (
            "worktree /Users/alex/repos/dropset\n"
            "HEAD abc\n"
            "branch refs/heads/main\n"
            "\n"
            "worktree /Users/alex/repos/dropset/.claude/worktrees/eng-663\n"
            "HEAD def\n"
            "branch refs/heads/eng-663\n"
        )
        self.assertEqual(
            parse_worktrees(porcelain),
            [
                ("/Users/alex/repos/dropset", "main"),
                ("/Users/alex/repos/dropset/.claude/worktrees/eng-663", "eng-663"),
            ],
        )

    def test_detached_worktree_has_no_branch(self):
        porcelain = "worktree /tmp/wt\nHEAD abc\ndetached\n"
        self.assertEqual(parse_worktrees(porcelain), [("/tmp/wt", None)])


class DropsetSlugSetsTests(unittest.TestCase):
    def test_forward_derivation_and_protection(self):
        worktrees = [
            ("/Users/alex/repos/dropset", "main"),
            ("/Users/alex/repos/dropset/.claude/worktrees/eng-663", "eng-663"),
        ]
        dropset, protected = dropset_slug_sets(worktrees, {"eng-663"})
        self.assertIn(slugify(Path("/Users/alex/repos/dropset")), dropset)
        self.assertIn(
            slugify(Path("/Users/alex/repos/dropset/.claude/worktrees/eng-663")),
            dropset,
        )
        # only the open-PR branch's slug is protected
        self.assertEqual(
            protected,
            {slugify(Path("/Users/alex/repos/dropset/.claude/worktrees/eng-663"))},
        )

    def test_sibling_repo_not_swept_in(self):
        # dropset-beta is a *different* repo; its slug starts with the base
        # repo's slug but must NOT be in the dropset set (forward derivation,
        # not prefix matching). It simply never appears in dropset's worktrees.
        worktrees = [("/Users/alex/repos/dropset", "main")]
        dropset, _ = dropset_slug_sets(worktrees, set())
        self.assertNotIn(slugify(Path("/Users/alex/repos/dropset-beta")), dropset)


class DecideSlugTests(unittest.TestCase):
    def _decide(self, slug, mtime, dropset, protected, current):
        return decide_slug(
            slug,
            mtime,
            dropset_slugs=dropset,
            protected_slugs=protected,
            current_slug=current,
            cutoff_ts=CUTOFF,
        )

    def test_current_slug_always_kept(self):
        d = self._decide("cur", OLD, {"cur"}, set(), "cur")
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "current session")

    def test_dropset_open_pr_kept_regardless_of_age(self):
        d = self._decide("d", OLD, {"d"}, {"d"}, None)
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "open PR")

    def test_dropset_aged_no_pr_deleted(self):
        d = self._decide("d", OLD, {"d"}, set(), None)
        self.assertTrue(d.delete)
        self.assertEqual(d.category, "dropset-old")

    def test_dropset_fresh_kept(self):
        d = self._decide("d", FRESH, {"d"}, set(), None)
        self.assertFalse(d.delete)

    def test_non_dropset_aged_deleted(self):
        d = self._decide("x", OLD, {"d"}, set(), None)
        self.assertTrue(d.delete)
        self.assertEqual(d.category, "non-dropset")

    def test_non_dropset_fresh_kept(self):
        d = self._decide("x", FRESH, {"d"}, set(), None)
        self.assertFalse(d.delete)


class DecideHistoryTests(unittest.TestCase):
    def test_current_uuid_kept(self):
        d = decide_history("uuid-1", OLD, current_uuid="uuid-1", cutoff_ts=CUTOFF)
        self.assertFalse(d.delete)

    def test_aged_deleted(self):
        d = decide_history("uuid-2", OLD, current_uuid="uuid-1", cutoff_ts=CUTOFF)
        self.assertTrue(d.delete)
        self.assertEqual(d.category, "file-history")

    def test_fresh_kept(self):
        d = decide_history("uuid-2", FRESH, current_uuid="uuid-1", cutoff_ts=CUTOFF)
        self.assertFalse(d.delete)


class IsWithinTests(unittest.TestCase):
    def test_under_root_true_escape_false(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            (root / "child").mkdir(parents=True)
            self.assertTrue(is_within(root, root / "child"))
            # a sibling outside the root is rejected
            outside = Path(tmp) / "outside"
            outside.mkdir()
            self.assertFalse(is_within(root, outside))
            # the root itself is not "under" the root
            self.assertFalse(is_within(root, root))


if __name__ == "__main__":
    unittest.main()
