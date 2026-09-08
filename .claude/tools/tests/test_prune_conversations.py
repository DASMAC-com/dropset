"""Stdlib ``unittest`` tests for prune_conversations' pure decision logic —
the age/open-PR rule, the live-worktree and recent-activity protections,
worktree parsing, the dropset-set derivation, the under-root path guard, the
invocation guards that refuse an unprotected run, and the destructive
``safe_delete`` guard's refusal branches. Run via the repo's ``make
tools-tests``.
"""

import io
import json
import os
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

import prune_conversations as pc

from prune_conversations import (
    PruneError,
    Record,
    base_worktree,
    decide_history,
    decide_slug,
    dropset_slug_sets,
    former_worktree_prefix,
    is_within,
    kept_by_reason,
    parse_worktrees,
    read_active_slugs,
    read_worktrees,
    render_manifest,
    resolve_dropset_repo,
    safe_delete,
    session_uuids_in,
    slugify,
    worktree_tag,
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
        live, protected = dropset_slug_sets(worktrees, {"eng-663"})
        self.assertIn(slugify(Path("/repos/dropset")), live)
        self.assertIn(
            slugify(Path("/repos/dropset/.claude/worktrees/eng-663")),
            live,
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
        live, _ = dropset_slug_sets(worktrees, set())
        self.assertNotIn(slugify(Path("/repos/dropset-beta")), live)


class DraftPrWorktreeFixtureTests(unittest.TestCase):
    """End-to-end over the real fixture shape: porcelain → slug sets → decision.

    The filed requirement names a worktree list plus a **draft** PR, so this
    walks the whole path rather than calling ``decide_slug`` directly — the
    2026-09-07 loss happened between those stages (an empty worktree list meant
    the protected branch mapped to no slug), which a unit test of the decision
    alone cannot reach.

    Note what "draft" contributes here: nothing at this layer, deliberately.
    The REST ``state`` filter is open/closed only, with ``draft`` a separate
    field, so a draft PR reaches the tool as an ordinary protected branch —
    verified against a live draft PR. The original filing's theory that drafts
    were being excluded was wrong, and the test that would have caught the real
    bug is the second one below: no PR at all, and the slug still survives.
    """

    PORCELAIN = (
        "worktree /repos/dropset\n"
        "HEAD abc\n"
        "branch refs/heads/main\n"
        "\n"
        "worktree /repos/dropset/.claude/worktrees/eng-1192\n"
        "HEAD def\n"
        "branch refs/heads/eng-1192\n"
    )
    SLUG = slugify(Path("/repos/dropset/.claude/worktrees/eng-1192"))

    def _decide(self, protected_branches):
        worktrees = parse_worktrees(self.PORCELAIN)
        live, protected = dropset_slug_sets(worktrees, protected_branches)
        return decide_slug(
            self.SLUG,
            OLD,
            live_slugs=live,
            protected_slugs=protected,
            current_slug=None,
            cutoff_ts=CUTOFF,
            former_prefix=former_worktree_prefix(base_worktree(worktrees)),
        )

    def test_a_branch_with_a_draft_pr_is_kept(self):
        decision = self._decide({"eng-1192"})
        self.assertFalse(decision.delete)
        self.assertEqual(decision.reason, "open PR")

    def test_the_slug_survives_even_when_the_pr_lookup_returns_nothing(self):
        # The actual 2026-09-07 shape: the protection list arrives empty. Under
        # the old rule this deleted; the live worktree now carries it alone.
        decision = self._decide(set())
        self.assertFalse(decision.delete)
        self.assertEqual(decision.reason, "live worktree")


class DecideSlugTests(unittest.TestCase):
    def _decide(self, slug, mtime, live, protected, current):
        return decide_slug(
            slug,
            mtime,
            live_slugs=live,
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

    def test_a_live_worktree_is_kept_when_aged_with_no_open_pr(self):
        # The regression test for the 2026-09-07 loss. This case used to delete
        # ("dropset-old"): a worktree that exists on disk was age-ruled, and its
        # only protection was an open-PR list supplied over the network. An
        # existing worktree now protects itself, so no lookup failure — and no
        # omitted argument — can reach a live session's transcript.
        d = self._decide("d", OLD, {"d"}, set(), None)
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "live worktree")

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

    def test_recent_prompt_activity_keeps_a_slug_with_no_other_protection(self):
        # The guard that survives a total failure of the PR lookup: no live
        # worktree, no protected branch, aged past the cutoff — kept anyway,
        # because the operator was prompting there.
        d = decide_slug(
            "busy",
            OLD,
            live_slugs=set(),
            protected_slugs=set(),
            current_slug=None,
            cutoff_ts=CUTOFF,
            active_slugs={"busy"},
        )
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "recent session activity")

    def test_recent_activity_beats_a_completed_marking(self):
        # Completion is the caller's set arithmetic; prompts are the operator's
        # own behavior. Believing the human costs disk; believing the
        # arithmetic can cost a transcript.
        d = decide_slug(
            "both",
            OLD,
            live_slugs=set(),
            protected_slugs=set(),
            current_slug=None,
            cutoff_ts=CUTOFF,
            completed_slugs={"both"},
            active_slugs={"both"},
        )
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "recent session activity")

    def test_a_pruned_away_worktree_is_labelled_dropset_not_non_dropset(self):
        # Same delete decision as before, different label. "non-dropset
        # transcripts" is what a human waves through, and mislabelling is what
        # made the real loss approvable.
        prefix = former_worktree_prefix(Path("/repos/dropset"))
        d = decide_slug(
            prefix + "eng-1192",
            OLD,
            live_slugs=set(),
            protected_slugs=set(),
            current_slug=None,
            cutoff_ts=CUTOFF,
            former_prefix=prefix,
        )
        self.assertTrue(d.delete)
        self.assertEqual(d.category, "dropset-old")

    def test_a_pruned_away_worktree_within_age_is_kept(self):
        prefix = former_worktree_prefix(Path("/repos/dropset"))
        d = decide_slug(
            prefix + "eng-1192",
            FRESH,
            live_slugs=set(),
            protected_slugs=set(),
            current_slug=None,
            cutoff_ts=CUTOFF,
            former_prefix=prefix,
        )
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "worktree gone, within age")

    def test_completed_work_skips_the_age_grace_period(self):
        # The whole point: a worktree that is gone and a PR that is merged is
        # finished, so a two-day grace period protects nothing.
        d = decide_slug(
            "done",
            FRESH,
            live_slugs=set(),
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
            live_slugs={"both"},
            protected_slugs={"both"},
            current_slug=None,
            cutoff_ts=CUTOFF,
            completed_slugs={"both"},
        )
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "open PR")

    def test_a_live_worktree_beats_a_completed_marking(self):
        # Reachable in practice: the skill computes "completed" from PR state,
        # which says nothing about whether the checkout is still on disk.
        d = decide_slug(
            "both",
            OLD,
            live_slugs={"both"},
            protected_slugs=set(),
            current_slug=None,
            cutoff_ts=CUTOFF,
            completed_slugs={"both"},
        )
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "live worktree")

    def test_an_open_pr_protects_a_slug_even_outside_the_live_set(self):
        # The open-PR check sits above the live-worktree branch, so the
        # guarantee is "an open PR is never pruned" rather than "…if we also
        # recognized its worktree".
        #
        # A NO-OP with today's caller, asserted so the ordering is deliberate
        # rather than an accident: `dropset_slug_sets` builds `protected` as a
        # strict subset of `live` from one loop, so this state is unreachable
        # through `run()`. It would matter only if protected slugs ever came
        # from a source other than the worktree list. Do not re-describe this as
        # protecting a pruned-away worktree — such a branch contributes no
        # worktree entry, so it cannot reach `protected` either.
        d = self._decide("x", OLD, set(), {"x"}, None)
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "open PR")

    def test_the_current_session_beats_a_completed_marking(self):
        d = decide_slug(
            "cur",
            OLD,
            live_slugs=set(),
            protected_slugs=set(),
            current_slug="cur",
            cutoff_ts=CUTOFF,
            completed_slugs={"cur"},
        )
        self.assertFalse(d.delete)

    def test_the_new_optional_sets_default_to_inert(self):
        # active_slugs and former_prefix both default to None, so a caller that
        # passes neither gets the plain live/age behavior.
        d = self._decide("d", FRESH, {"d"}, set(), None)
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "live worktree")
        d = self._decide("x", OLD, set(), set(), None)
        self.assertTrue(d.delete)
        self.assertEqual(d.category, "non-dropset")


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

    def test_a_session_of_a_kept_project_survives_the_age_rule(self):
        # The other half of the 2026-09-07 loss: file-history was age-only, so
        # protecting the transcript alone would still leave the same session
        # half-destroyable.
        d = decide_history(
            "uuid-2",
            OLD,
            current_uuid=None,
            cutoff_ts=CUTOFF,
            protected_uuids={"uuid-2"},
        )
        self.assertFalse(d.delete)
        self.assertEqual(d.reason, "session of a kept project")

    def test_an_unprotected_session_still_ages_out(self):
        d = decide_history(
            "uuid-3",
            OLD,
            current_uuid=None,
            cutoff_ts=CUTOFF,
            protected_uuids={"uuid-2"},
        )
        self.assertTrue(d.delete)


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


class SessionUuidsInTests(unittest.TestCase):
    def test_reads_transcript_stems_and_ignores_other_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            slug = Path(tmp)
            (slug / "aaa-111.jsonl").write_text("{}\n", encoding="utf-8")
            (slug / "bbb-222.jsonl").write_text("{}\n", encoding="utf-8")
            (slug / "notes.txt").write_text("x", encoding="utf-8")
            (slug / "sub").mkdir()
            self.assertEqual(session_uuids_in(slug), {"aaa-111", "bbb-222"})

    def test_a_missing_directory_is_not_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(session_uuids_in(Path(tmp) / "nope"), set())


class RunWiringTests(unittest.TestCase):
    """End-to-end over ``run()`` against a synthetic ``~/.claude`` tree.

    Every other test here exercises a pure function in isolation. That is not
    enough for this diff, because the 2026-09-07 loss did not happen inside a
    decision function — it happened in the **wiring**, where an omitted argument
    meant the worktree list never reached the decisions at all. Each helper can
    be individually correct while nothing connects them, and a unit-only suite
    stays green through exactly that.

    So this pins the connections: worktree list -> live/protected slug sets ->
    former-worktree prefix -> per-entry tag -> kept projects -> the session
    UUIDs those projects hold -> file-history protection -> the manifest.
    Reverting any one of those wiring lines fails this test.
    """

    BASE = "/repos/dropset"
    LIVE = "/repos/dropset/.claude/worktrees/eng-1192"
    WORKTREES = [(BASE, "main"), (LIVE, "eng-1192")]
    NOW = 2_000_000.0
    AGED = NOW - 10_000  # older than a zero-day cutoff

    def _tree(self, tmp):
        """Build the three roots, all aged well past any cutoff."""
        home = Path(tmp) / "claude"
        cache = Path(tmp) / "cache"
        projects = home / "projects"
        history = home / "file-history"
        for d in (projects, history, cache):
            d.mkdir(parents=True)

        prefix = pc.former_worktree_prefix(Path(self.BASE))
        made = {
            "live": projects / slugify(Path(self.LIVE)),
            "gone": projects / f"{prefix}eng-999",
            "foreign": projects / "-repos-elsewhere",
            "busy": projects / "-repos-busy",
        }
        for path in made.values():
            path.mkdir()
        # The live worktree's transcript — this is what joins it to file-history.
        (made["live"] / "live-session.jsonl").write_text("{}\n", encoding="utf-8")
        (history / "live-session").mkdir()
        (history / "orphan-session").mkdir()

        # A prompt in "busy" newer than the cutoff, so the activity guard fires.
        (home / "history.jsonl").write_text(
            json.dumps({"project": "/repos/busy", "timestamp": self.NOW * 1000}) + "\n",
            encoding="utf-8",
        )

        for path in list(made.values()) + [
            history / "live-session",
            history / "orphan-session",
        ]:
            os.utime(path, (self.AGED, self.AGED))
        return home, cache

    def _run(self, tmp, argv=(), keep_mocks=False):
        home, cache = self._tree(tmp)
        buf = io.StringIO()
        with (
            mock.patch.object(pc, "claude_home", return_value=home),
            mock.patch.object(pc, "cli_cache_root", return_value=cache),
            mock.patch.object(
                pc, "resolve_dropset_repo", return_value=self.BASE
            ) as resolve,
            mock.patch.object(
                pc, "read_worktrees", return_value=self.WORKTREES
            ) as read,
            redirect_stdout(buf),
        ):
            code = pc.run(
                [
                    "prune_conversations.py",
                    "--age-days",
                    "0",
                    "--now",
                    str(self.NOW),
                    *argv,
                ]
            )
        self.assertEqual(code, 0)
        return (buf.getvalue(), resolve, read) if keep_mocks else buf.getvalue()

    def test_the_resolved_repo_is_what_reaches_the_worktree_lookup(self):
        """The one wiring link the rest of this class cannot see.

        Both endpoints are stubbed, so asserting only on the printed manifest
        leaves this link untested — and it is the link the 2026-09-07 loss ran
        through. Reverting ``read_worktrees(repo)`` to
        ``read_worktrees(args.dropset_repo)``, or dropping the
        ``resolve_dropset_repo`` call entirely, reproduces the incident
        invocation exactly while every output assertion here still passes. So
        assert the call arguments, not just the output.
        """
        with tempfile.TemporaryDirectory() as tmp:
            _out, resolve, read = self._run(
                tmp, argv=("--dropset-repo", "/passed/explicitly"), keep_mocks=True
            )
        # The parsed flag reaches the resolver...
        resolve.assert_called_once_with("/passed/explicitly")
        # ...and the RESOLVED value, not the raw flag, reaches the lookup.
        read.assert_called_once_with(self.BASE)

    def test_a_live_worktrees_transcript_and_file_history_both_survive(self):
        # The whole point of the fix, asserted through the wiring rather than
        # through decide_slug/decide_history in isolation.
        with tempfile.TemporaryDirectory() as tmp:
            out = self._run(tmp)
        self.assertIn("live worktree: 1", out)
        self.assertIn("session of a kept project: 1", out)

    def test_the_activity_guard_reaches_the_decision(self):
        # Pins `read_active_slugs(cutoff_ts)` actually being computed and passed
        # through; dropping either leaves this slug aged and deletable.
        with tempfile.TemporaryDirectory() as tmp:
            out = self._run(tmp)
        self.assertIn("recent session activity: 1", out)

    def test_a_pruned_away_worktree_is_named_by_its_tag_in_the_manifest(self):
        # Pins the tag being assigned in scan_slug_root, not merely rendered by
        # render_manifest from a hand-built Record.
        with tempfile.TemporaryDirectory() as tmp:
            out = self._run(tmp)
        self.assertIn("dropset transcripts (worktree gone, aged)", out)
        self.assertIn("- eng-999", out)

    def test_an_untagged_deletion_is_still_named_in_the_manifest(self):
        # The security-lens gap: naming only tagged entries would leave exactly
        # the wrongly-resolved-repo case printing as an anonymous bulk line.
        with tempfile.TemporaryDirectory() as tmp:
            out = self._run(tmp)
        self.assertIn("- -repos-elsewhere", out)

    def test_an_orphan_file_history_dir_still_ages_out(self):
        # The protection must be a join, not a blanket keep: a session UUID with
        # no kept project behind it is still reclaimable.
        with tempfile.TemporaryDirectory() as tmp:
            out = self._run(tmp)
        self.assertIn("- orphan-session", out)


class BaseWorktreeTests(unittest.TestCase):
    def test_the_first_entry_is_the_main_worktree(self):
        worktrees = [
            ("/repos/dropset", "main"),
            ("/repos/dropset/.claude/worktrees/eng-663", "eng-663"),
        ]
        self.assertEqual(base_worktree(worktrees), Path("/repos/dropset"))

    def test_an_empty_list_raises_rather_than_guessing(self):
        with self.assertRaises(PruneError):
            base_worktree([])


class WorktreeTagTests(unittest.TestCase):
    def setUp(self):
        self.prefix = former_worktree_prefix(Path("/repos/dropset"))

    def test_the_prefix_reaches_inside_the_repo_so_a_sibling_cannot_match(self):
        # The reason this prefix comparison is safe where a bare base-repo
        # prefix is not: `dropset-beta` shares the base slug but cannot share a
        # directory that lives inside dropset.
        self.assertNotIn(
            slugify(Path("/repos/dropset-beta")),
            self.prefix,
        )
        self.assertFalse(
            slugify(Path("/repos/dropset-beta/.claude/worktrees/eng-1")).startswith(
                self.prefix
            )
        )

    def test_extracts_the_issue_tag(self):
        slug = slugify(Path("/repos/dropset/.claude/worktrees/eng-1192"))
        self.assertEqual(worktree_tag(slug, self.prefix), "eng-1192")

    def test_returns_none_for_a_foreign_slug(self):
        self.assertIsNone(worktree_tag(slugify(Path("/repos/other")), self.prefix))

    def test_returns_none_without_a_prefix(self):
        self.assertIsNone(worktree_tag("anything", None))


class ReadActiveSlugsTests(unittest.TestCase):
    def _write(self, tmp, records):
        path = Path(tmp) / "history.jsonl"
        path.write_text(
            "".join(json.dumps(r) + "\n" for r in records), encoding="utf-8"
        )
        return path

    def test_recent_projects_become_slugs_and_old_ones_do_not(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write(
                tmp,
                [
                    {"project": "/repos/dropset", "timestamp": FRESH * 1000},
                    {"project": "/repos/stale", "timestamp": OLD * 1000},
                ],
            )
            slugs = read_active_slugs(CUTOFF, path)
            self.assertIn(slugify(Path("/repos/dropset")), slugs)
            self.assertNotIn(slugify(Path("/repos/stale")), slugs)

    def test_a_missing_file_is_not_an_error(self):
        # Best-effort by construction: a guard that can abort the run would
        # itself become a reason to skip the guard.
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(read_active_slugs(CUTOFF, Path(tmp) / "nope.jsonl"), set())

    def test_malformed_lines_are_skipped_without_losing_good_ones(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "history.jsonl"
            path.write_text(
                "not json\n"
                "[]\n"
                '{"project": 42, "timestamp": 1}\n'
                '{"timestamp": 1}\n'
                '{"project": "/repos/dropset"}\n'
                '{"project": "/repos/dropset", "timestamp": true}\n'
                + json.dumps({"project": "/repos/good", "timestamp": FRESH * 1000})
                + "\n",
                encoding="utf-8",
            )
            self.assertEqual(
                read_active_slugs(CUTOFF, path),
                {slugify(Path("/repos/good"))},
            )


class InvocationGuardTests(unittest.TestCase):
    """The two guards that close the hole an omitted --dropset-repo opened."""

    def test_an_explicit_repo_is_honored(self):
        # It is still resolved through git (so a worktree normalizes to its own
        # root) and still sanity-checked, but the caller's choice is what is
        # looked up — the default is never substituted for it.
        root = str(pc.repo_root_from_tool())
        self.assertEqual(
            Path(resolve_dropset_repo(root)).resolve(), Path(root).resolve()
        )

    def test_the_default_ignores_the_working_directory_entirely(self):
        # The security-lens finding: defaulting from the cwd meant running from
        # inside ANY other git checkout resolved the wrong repo, silently, and
        # classified every dropset slug as "non-dropset, older than threshold" —
        # the exact label that made the original loss approvable. Deriving the
        # default from the tool's own committed location cannot be wrong about
        # which repo is meant, so the cwd must not affect it.
        expected = str(pc.repo_root_from_tool())
        with tempfile.TemporaryDirectory() as tmp:
            cwd = os.getcwd()
            os.chdir(tmp)
            try:
                from_tempdir = resolve_dropset_repo(None)
            finally:
                os.chdir(cwd)
        from_repo = resolve_dropset_repo(None)
        self.assertEqual(from_tempdir, from_repo)
        self.assertEqual(Path(from_tempdir).resolve(), Path(expected).resolve())

    def test_a_repo_without_a_claude_directory_is_refused(self):
        # The sanity gate behind an explicit --dropset-repo typo: honor the
        # caller's intent, but refuse a path that cannot be the repo this tool
        # protects rather than computing protections that would match nothing.
        with tempfile.TemporaryDirectory() as tmp:
            subprocess.run(["git", "-C", tmp, "init", "-q"], check=True)
            with self.assertRaises(PruneError) as caught:
                resolve_dropset_repo(tmp)
        self.assertIn(".claude", str(caught.exception))

    def test_an_explicit_path_naming_a_different_repo_is_refused(self):
        # The mandated path is the less-guarded one: the skill tells operators
        # to always pass --dropset-repo, so it is where a typo lands, and a
        # .claude check alone accepts any other Claude-using checkout. Compare
        # by --git-common-dir, which every worktree of a repo shares.
        with tempfile.TemporaryDirectory() as tmp:
            other = Path(tmp) / "other"
            (other / ".claude").mkdir(parents=True)
            subprocess.run(["git", "-C", str(other), "init", "-q"], check=True)
            with self.assertRaises(PruneError) as caught:
                resolve_dropset_repo(str(other))
        message = str(caught.exception)
        self.assertIn("different repository", message)
        # The remedy has to be actionable, since a second clone hits this too.
        self.assertIn("run its own copy", message)

    def test_a_worktree_of_this_repo_is_accepted_explicitly(self):
        # The gate must not reject the legitimate case it most resembles: this
        # session runs from a linked worktree, whose common dir is the base
        # repo's, so passing either must work.
        root = str(pc.repo_root_from_tool())
        self.assertTrue(resolve_dropset_repo(root))

    def test_an_explicit_non_repo_path_aborts(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(PruneError) as caught:
                resolve_dropset_repo(tmp)
        message = str(caught.exception)
        self.assertIn("not inside a git worktree", message)
        # The remedy must match the path taken: telling a caller who passed
        # --dropset-repo to pass --dropset-repo reads as a bug in the tool.
        self.assertIn("Check the path you passed", message)
        self.assertNotIn("Pass --dropset-repo", message)

    def test_a_path_that_is_not_a_repo_aborts(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(PruneError):
                read_worktrees(tmp)

    def test_an_empty_worktree_listing_aborts_instead_of_protecting_nothing(self):
        # `git worktree list` always reports at least the main worktree, so an
        # empty result means the lookup did not do what the caller thinks — the
        # exact state that deleted a live session. Reaching that branch needs a
        # stubbed git, because real git cannot produce it; the guard is
        # defense-in-depth behind resolve_dropset_repo, and worth asserting for
        # exactly that reason (nothing else would catch it regressing).
        completed = subprocess.CompletedProcess(args=[], returncode=0, stdout="")
        with mock.patch("prune_conversations.subprocess.run", return_value=completed):
            with self.assertRaises(PruneError) as caught:
                read_worktrees("/repos/dropset")
        self.assertIn("refusing to run", str(caught.exception))


class ManifestNamingTests(unittest.TestCase):
    def test_every_tagged_deletion_is_named_in_the_manifest(self):
        groups = {
            "dropset-old": [
                Record(
                    Path("/p/a"), "dropset-old", True, "aged", 2_000_000, "eng-1192"
                ),
                Record(Path("/p/b"), "dropset-old", True, "aged", 1_000_000, "eng-800"),
            ]
        }
        out = render_manifest(groups, 0)
        self.assertIn("eng-1192", out)
        self.assertIn("eng-800", out)

    def test_an_untagged_deletion_is_named_by_its_directory(self):
        groups = {
            "non-dropset": [
                Record(Path("/p/x"), "non-dropset", True, "aged", 1_000_000)
            ]
        }
        out = render_manifest(groups, 0)
        self.assertIn("non-dropset transcripts: 1 dir(s)", out)
        self.assertIn("    - x ", out)

    def test_tagged_entries_sort_ahead_of_untagged_ones(self):
        # So the cap can never hide a dropset session behind foreign slugs.
        groups = {
            "non-dropset": [
                Record(Path("/p/aaa"), "non-dropset", True, "aged", 1, None),
                Record(Path("/p/zzz"), "non-dropset", True, "aged", 1, "eng-1192"),
            ]
        }
        out = render_manifest(groups, 0)
        self.assertLess(out.index("- eng-1192"), out.index("- aaa"))

    def test_tagged_entries_are_never_capped(self):
        # Sorting tagged first is NOT sufficient on its own: a repo accumulates
        # per-issue worktrees, so a group can hold more tagged entries than the
        # cap, and truncating those drops real session names — in arbitrary
        # lexical order — from a hard-delete approval prompt. That is the exact
        # hole naming entries was added to close.
        count = pc.MANIFEST_NAME_CAP + 5
        groups = {
            "dropset-old": [
                Record(Path(f"/p/w{i:03d}"), "dropset-old", True, "aged", 1, f"eng-{i}")
                for i in range(count)
            ]
        }
        out = render_manifest(groups, 0)
        for i in range(count):
            self.assertIn(f"- eng-{i} ", out)
        self.assertNotIn("more", out.split("TOTAL")[0])

    def test_a_long_group_is_capped_with_a_remainder_line(self):
        count = pc.MANIFEST_NAME_CAP + 3
        groups = {
            "non-dropset": [
                Record(Path(f"/p/slug{i:03d}"), "non-dropset", True, "aged", 1)
                for i in range(count)
            ]
        }
        out = render_manifest(groups, 0)
        self.assertIn("… and 3 more", out)
        self.assertIn("- slug000", out)
        self.assertNotIn(f"- slug{count - 1:03d}", out)


if __name__ == "__main__":
    unittest.main()
