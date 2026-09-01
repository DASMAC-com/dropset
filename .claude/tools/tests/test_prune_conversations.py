"""Stdlib ``unittest`` tests for prune_conversations' pure decision logic —
the age/open-PR rule, worktree parsing, the dropset-set derivation, the
under-root path guard, and the destructive ``safe_delete`` guard's refusal
branches. Run via the repo's ``make tools-tests``.
"""

import os
import tempfile
import unittest
from pathlib import Path

from prune_conversations import (
    Record,
    decide_history,
    decide_slug,
    dropset_slug_sets,
    is_within,
    kept_by_reason,
    parse_worktrees,
    safe_delete,
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
        self.assertEqual(slugify(Path("/repos/dropset")), "-repos-dropset")
        self.assertEqual(
            slugify(Path("/a/.claude/worktrees/eng-663")),
            "-a--claude-worktrees-eng-663",
        )


class ParseWorktreesTests(unittest.TestCase):
    def test_parses_paths_and_short_branches(self):
        porcelain = (
            "worktree /repos/dropset\n"
            "HEAD abc\n"
            "branch refs/heads/main\n"
            "\n"
            "worktree /repos/dropset/.claude/worktrees/eng-663\n"
            "HEAD def\n"
            "branch refs/heads/eng-663\n"
        )
        self.assertEqual(
            parse_worktrees(porcelain),
            [
                ("/repos/dropset", "main"),
                ("/repos/dropset/.claude/worktrees/eng-663", "eng-663"),
            ],
        )

    def test_detached_worktree_has_no_branch(self):
        porcelain = "worktree /tmp/wt\nHEAD abc\ndetached\n"
        self.assertEqual(parse_worktrees(porcelain), [("/tmp/wt", None)])


class DropsetSlugSetsTests(unittest.TestCase):
    def test_forward_derivation_and_protection(self):
        worktrees = [
            ("/repos/dropset", "main"),
            ("/repos/dropset/.claude/worktrees/eng-663", "eng-663"),
        ]
        dropset, protected = dropset_slug_sets(worktrees, {"eng-663"})
        self.assertIn(slugify(Path("/repos/dropset")), dropset)
        self.assertIn(
            slugify(Path("/repos/dropset/.claude/worktrees/eng-663")),
            dropset,
        )
        # only the open-PR branch's slug is protected
        self.assertEqual(
            protected,
            {slugify(Path("/repos/dropset/.claude/worktrees/eng-663"))},
        )

    def test_sibling_repo_not_swept_in(self):
        # dropset-beta is a *different* repo; its slug starts with the base
        # repo's slug but must NOT be in the dropset set (forward derivation,
        # not prefix matching). It simply never appears in dropset's worktrees.
        worktrees = [("/repos/dropset", "main")]
        dropset, _ = dropset_slug_sets(worktrees, set())
        self.assertNotIn(slugify(Path("/repos/dropset-beta")), dropset)


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

    def test_completed_work_skips_the_age_grace_period(self):
        # The whole point: a worktree that is gone and a PR that is merged is
        # finished, so a two-day grace period protects nothing.
        d = decide_slug(
            "done",
            FRESH,
            dropset_slugs=set(),
            protected_slugs=set(),
            current_slug=None,
            cutoff_ts=CUTOFF,
            completed_slugs={"done"},
        )
        self.assertTrue(d.delete)
        self.assertEqual(d.category, "completed")

    def test_an_open_pr_beats_a_completed_marking(self):
        # They are mutually exclusive by construction, so ordering it this way
        # means a bug in the caller's set arithmetic costs disk, not data.
        d = decide_slug(
            "both",
            OLD,
            dropset_slugs={"both"},
            protected_slugs={"both"},
            current_slug=None,
            cutoff_ts=CUTOFF,
            completed_slugs={"both"},
        )
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "open PR")

    def test_an_open_pr_protects_a_slug_even_outside_the_dropset_set(self):
        # The open-PR check sits above the dropset branch, so the guarantee is
        # "an open PR is never pruned" rather than "…if we also recognized its
        # worktree". A no-op with today's caller, asserted so the widening is
        # deliberate rather than an accident of the reordering.
        d = self._decide("x", OLD, set(), {"x"}, None)
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "open PR")

    def test_the_current_session_beats_a_completed_marking(self):
        d = decide_slug(
            "cur",
            OLD,
            dropset_slugs=set(),
            protected_slugs=set(),
            current_slug="cur",
            cutoff_ts=CUTOFF,
            completed_slugs={"cur"},
        )
        self.assertFalse(d.delete)

    def test_omitting_completed_slugs_changes_nothing(self):
        # The parameter defaults to None, so every existing caller keeps its
        # exact prior behavior.
        d = self._decide("d", FRESH, {"d"}, set(), None)
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "dropset, within age")


class KeptByReasonTests(unittest.TestCase):
    """One collapsed 'protected' figure read as open-PR protection when only
    four of 41 records actually were — the rest were the blunt age rule."""

    @staticmethod
    def _record(reason, delete=False):
        return Record(Path("/x"), "kept", delete, reason, 0)

    def test_kept_records_are_counted_per_reason(self):
        groups = {
            "kept": [
                self._record("open PR"),
                self._record("open PR"),
                self._record("dropset, within age"),
            ]
        }
        self.assertEqual(
            kept_by_reason(groups),
            {"dropset, within age": 1, "open PR": 2},
        )

    def test_deleted_records_are_not_counted_as_kept(self):
        groups = {"dropset-old": [self._record("older than threshold", delete=True)]}
        self.assertEqual(kept_by_reason(groups), {})


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


class SafeDeleteTests(unittest.TestCase):
    """The one `rmtree` caller: it must delete a real directory under a known
    root and **refuse** anything else (symlink, non-dir, outside every root)."""

    def _record(self, path, size=123):
        return Record(
            path=path, category="dropset-old", delete=True, reason="", size=size
        )

    def test_deletes_real_dir_under_root_and_returns_size(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            victim = root / "slug"
            victim.mkdir(parents=True)
            (victim / "f").write_text("x", encoding="utf-8")
            freed = safe_delete(self._record(victim, size=999), [root])
            self.assertEqual(freed, 999)
            self.assertFalse(victim.exists())

    def test_refuses_outside_every_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            root.mkdir()
            outside = Path(tmp) / "outside"
            outside.mkdir()
            freed = safe_delete(self._record(outside), [root])
            self.assertEqual(freed, 0)
            self.assertTrue(outside.exists())  # untouched

    def test_refuses_symlink_entry(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            root.mkdir()
            real = Path(tmp) / "real"
            real.mkdir()
            link = root / "link"
            os.symlink(real, link)
            freed = safe_delete(self._record(link), [root])
            self.assertEqual(freed, 0)
            self.assertTrue(real.exists())  # symlink target never followed/deleted
            self.assertTrue(link.is_symlink())

    def test_refuses_missing_or_non_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            root.mkdir()
            missing = root / "gone"
            self.assertEqual(safe_delete(self._record(missing), [root]), 0)


if __name__ == "__main__":
    unittest.main()
