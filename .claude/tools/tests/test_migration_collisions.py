#!/usr/bin/env python3
"""Unit tests for ``migration_collisions.py`` (stdlib ``unittest``; no pytest).

The property that matters is that collisions are detected by **version number**
rather than by filename — the observed real instance was two PRs adding
``0003_telemetry.sql`` and ``0003_roster.sql``, which a path comparison misses
entirely.
"""

from __future__ import annotations

import io
import json
import os
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

import migration_collisions as mc


class MigrationNumber(unittest.TestCase):
    def test_it_reads_the_leading_version(self):
        self.assertEqual(mc.migration_number("db-schema/migrations/0004_x.sql"), 4)
        self.assertEqual(mc.migration_number("0012_y.sql"), 12)

    def test_a_non_migration_path_yields_none(self):
        # A caller may pass a whole file list, so this must not raise.
        self.assertIsNone(mc.migration_number("db-schema/README.md"))
        self.assertIsNone(mc.migration_number("no_leading_digits.sql"))

    def test_the_number_is_anchored_at_the_basename(self):
        # A directory component starting with digits must not be read as the
        # version.
        self.assertEqual(mc.migration_number("2024_old/0007_z.sql"), 7)
        self.assertIsNone(mc.migration_number("0007_dir/plain.sql"))

    def test_a_sidecar_sharing_a_migrations_version_prefix_is_not_a_migration(self):
        # The migrations directory also holds a `<version>_<name>.fence`
        # manifest beside each migration. Matching the version prefix alone read
        # every one of them as an added migration, which invents a collision and
        # blocks an enqueue on a branch that touched no SQL.
        self.assertIsNone(
            mc.migration_number("db-schema/migrations/0009_instruments.fence")
        )
        self.assertEqual(
            mc.migration_number("db-schema/migrations/0009_instruments.sql"), 9
        )


class Collisions(unittest.TestCase):
    def test_the_real_instance_two_different_names_one_number(self):
        mine = ["db-schema/migrations/0003_maker_telemetry.sql"]
        others = [{"pr": 351, "files": ["db-schema/migrations/0003_pyth_roster.sql"]}]
        found = mc.collisions(mine, others)
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0]["number"], 3)
        self.assertEqual(found[0]["pr"], 351)

    def test_distinct_numbers_do_not_collide(self):
        mine = ["db-schema/migrations/0004_a.sql"]
        others = [{"pr": 1, "files": ["db-schema/migrations/0005_b.sql"]}]
        self.assertEqual(mc.collisions(mine, others), [])

    def test_an_other_pr_with_no_migrations_is_fine(self):
        mine = ["db-schema/migrations/0004_a.sql"]
        self.assertEqual(mc.collisions(mine, [{"pr": 1, "files": []}]), [])
        self.assertEqual(mc.collisions(mine, [{"pr": 1}]), [])

    def test_adding_no_migration_collides_with_nothing(self):
        others = [{"pr": 1, "files": ["db-schema/migrations/0004_b.sql"]}]
        self.assertEqual(mc.collisions([], others), [])

    def test_every_colliding_pr_is_reported_not_just_the_first(self):
        mine = ["db-schema/migrations/0003_a.sql"]
        others = [
            {"pr": 7, "files": ["db-schema/migrations/0003_b.sql"]},
            {"pr": 9, "files": ["db-schema/migrations/0003_c.sql"]},
        ]
        self.assertEqual([c["pr"] for c in mc.collisions(mine, others)], [7, 9])

    def test_non_migration_files_in_the_others_list_are_ignored(self):
        mine = ["db-schema/migrations/0004_a.sql"]
        others = [{"pr": 1, "files": ["db-schema/README.md", "src/lib.rs"]}]
        self.assertEqual(mc.collisions(mine, others), [])


class Summary(unittest.TestCase):
    def _result(
        self,
        mine,
        numbers,
        found,
        prs=2,
        self_branch="feature",
        self_prs=(),
        next_free=14,
        self_branch_explicit=False,
        self_fork_prs=(),
    ):
        return {
            "mine": mine,
            "mine_numbers": numbers,
            "prs_checked": prs,
            "self_branch": self_branch,
            "self_branch_explicit": self_branch_explicit,
            "self_prs": list(self_prs),
            "self_fork_prs": list(self_fork_prs),
            "collisions": found,
            "next_free_number": next_free,
            "clear": not found,
            "status": (
                "collision" if found else ("clear" if mine else "nothing_claimed")
            ),
        }

    def test_no_migration_says_so_without_claiming_to_be_clear(self):
        # The fail-open half of ENG-1336: this is the state `init-pr` calls the
        # tool in, before the migration file exists. Saying "nothing to check"
        # and nothing else reads as an all-clear, so the line has to disclaim
        # the verdict AND carry the number the caller actually came for.
        line = mc.summarize(self._result([], [], [], next_free=14))
        self.assertIn("adds no migration", line)
        self.assertIn("nothing was compared", line)
        self.assertIn("NOT a clear verdict", line)
        self.assertIn("0014", line)
        self.assertNotIn("safe to enqueue", line)

    def test_a_clear_verdict_names_the_excluded_self_pr(self):
        # The count is only trustworthy if the reader can tell whether their own
        # PR was among those compared.
        line = mc.summarize(
            self._result(["m/0004_a.sql"], [4], [], prs=2, self_prs=[427])
        )
        self.assertIn("excluding this branch's own PR #427", line)
        self.assertIn("safe to enqueue", line)

    def test_a_name_matched_fork_is_reported_so_its_collision_is_not_dismissed(self):
        # The fork is deliberately KEPT, so a collision it carries is a third
        # party's. Without a note the reader cannot tell it from a self-match,
        # which is the misread this whole issue removes.
        found = [{"number": 3, "pr": 999, "ours": "a", "theirs": "m/0003_b.sql"}]
        line = mc.summarize(
            self._result(
                ["m/0003_a.sql"], [3], found, self_prs=[427], self_fork_prs=[999]
            )
        )
        self.assertIn("#999", line)
        self.assertIn("fork", line)
        self.assertIn("NOT excluded", line)

    def test_the_nothing_claimed_line_still_reports_the_self_treatment(self):
        # `next_free_number` is shaped by the exclusion even though no comparison
        # ran, and this is the line `init-pr` reads — so an override must show up
        # here or the bypass is invisible where the number is taken from.
        line = mc.summarize(
            self._result([], [], [], self_prs=[427], self_branch_explicit=True)
        )
        self.assertIn("0014", line)
        self.assertIn("#427", line)
        self.assertIn("--self-branch", line)

    def test_a_detached_head_says_the_self_exclusion_was_not_applied(self):
        # Silence here would put the old fail-closed behavior back with no way
        # to recognize it from the output.
        line = mc.summarize(self._result(["m/0004_a.sql"], [4], [], self_branch=None))
        self.assertIn("detached HEAD", line)
        self.assertIn("not excluded", line)

    def test_a_clear_verdict_names_the_number_and_the_pr_count(self):
        line = mc.summarize(self._result(["m/0004_a.sql"], [4], []))
        self.assertIn("adds 4", line)
        self.assertIn("2 open PR(s)", line)
        self.assertIn("safe to enqueue", line)

    def test_a_collision_states_the_tiebreak_rule(self):
        found = [{"number": 3, "pr": 351, "ours": "a", "theirs": "m/0003_b.sql"}]
        line = mc.summarize(self._result(["m/0003_a.sql"], [3], found))
        self.assertIn("COLLISION", line)
        self.assertIn("do not", line)
        # The direction of the tiebreak is the load-bearing half: renumbering
        # the wrong branch wedges the shared dev database.
        self.assertIn("already applied", line)


class LoadOthers(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = self._tmp.name

    def _write(self, body: str) -> str:
        path = os.path.join(self.root, "others.json")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(body)
        return path

    def test_a_well_formed_payload_loads(self):
        path = self._write('[{"pr": 1, "files": ["m/0001_a.sql"]}]')
        self.assertEqual(mc.load_others(path)[0]["pr"], 1)

    def test_an_empty_list_is_valid(self):
        self.assertEqual(mc.load_others(self._write("[]")), [])

    def test_a_missing_file_is_a_clean_error(self):
        with self.assertRaises(mc.MigrationCollisionsError) as caught:
            mc.load_others(os.path.join(self.root, "gone.json"))
        self.assertIn("cannot read", str(caught.exception))

    def test_bad_json_is_a_clean_error(self):
        with self.assertRaises(mc.MigrationCollisionsError) as caught:
            mc.load_others(self._write("{not json"))
        self.assertIn("not valid JSON", str(caught.exception))

    def test_a_non_list_is_refused_rather_than_comparing_nothing(self):
        # Silently comparing against an empty set would report "clear" for a
        # branch that was never actually checked.
        with self.assertRaises(mc.MigrationCollisionsError):
            mc.load_others(self._write('{"pr": 1}'))

    def test_an_entry_without_a_pr_key_is_refused(self):
        with self.assertRaises(mc.MigrationCollisionsError) as caught:
            mc.load_others(self._write('[{"files": ["m/0001_a.sql"]}]'))
        self.assertIn("`pr`", str(caught.exception))

    def test_a_string_files_value_is_refused_rather_than_silently_clearing(self):
        # The fail-open shape: iterating a string yields characters, each of
        # which maps to no migration number, so nothing collides and the tool
        # would report `clear` for a PR it never actually compared.
        payload = '[{"pr": 351, "files": "db-schema/migrations/0003_pyth.sql"}]'
        with self.assertRaises(mc.MigrationCollisionsError) as caught:
            mc.load_others(self._write(payload))
        message = str(caught.exception)
        self.assertIn("must be a list", message)
        self.assertIn("351", message)

    def test_a_truthy_string_is_cross_repository_is_refused(self):
        # Read by truthiness, so `"false"` is true and would keep the PR from
        # ever being self-excluded -- silently restoring the fail-closed defect.
        path = self._write(
            '[{"pr": 1, "files": [], "isCrossRepository": "false"}]',
        )
        with self.assertRaises(mc.MigrationCollisionsError) as caught:
            mc.load_others(path)
        self.assertIn("isCrossRepository", str(caught.exception))

    def test_a_boolean_is_cross_repository_is_accepted(self):
        path = self._write('[{"pr": 1, "files": [], "isCrossRepository": true}]')
        self.assertEqual(mc.load_others(path)[0]["isCrossRepository"], True)

    def test_an_absent_or_empty_files_key_is_still_allowed(self):
        # The deliberate case the type check must NOT break: a PR touching no
        # migration at all.
        path = self._write('[{"pr": 1}, {"pr": 2, "files": []}]')
        self.assertEqual([e["pr"] for e in mc.load_others(path)], [1, 2])


class OthersFromGh(unittest.TestCase):
    """The in-process fetch. It exists because the two-command form had an
    unwritable gap — a redirect is a compound the shell guard blocks, and
    re-emitting the output with the Write tool routes every open PR's file
    list through context, which is the cost the tool exists to avoid."""

    def _gh(self, stdout, returncode=0, stderr=""):
        return mock.patch.object(
            mc.subprocess,
            "run",
            return_value=subprocess.CompletedProcess(
                args=mc.GH_OPEN_PRS,
                returncode=returncode,
                stdout=stdout,
                stderr=stderr,
            ),
        )

    def test_it_normalizes_ghs_shape_to_the_others_shape(self):
        """gh returns `number` and a list of {path: …}; downstream must see
        exactly one shape, so there is one comparison path."""
        payload = json.dumps(
            [
                {
                    "number": 351,
                    "headRefName": "eng-351",
                    "files": [
                        {"path": "db-schema/migrations/0004_pyth.sql"},
                        {"path": "README.md"},
                    ],
                }
            ]
        )
        with self._gh(payload):
            got = mc.others_from_gh()
        self.assertEqual(
            got,
            [
                {
                    "pr": 351,
                    "files": [
                        "db-schema/migrations/0004_pyth.sql",
                        "README.md",
                    ],
                    "headRefName": "eng-351",
                    "isCrossRepository": None,
                    "truncated": None,
                    "file_count": 2,
                }
            ],
        )

    def test_the_json_field_list_requests_what_the_compare_reads(self):
        # Self-exclusion is matched on the branch, so the field has to be in the
        # --json list. Without it every entry's headRefName is None, nothing is
        # ever excluded, and the fail-closed bug is silently back.
        #
        # Asserted against the element **following** `--json`, not against the
        # joined argv: a substring match over the whole command line also passes
        # when the token has drifted into a separate element, which `gh` would
        # either reject or silently not return the field for.
        argv = list(mc.GH_OPEN_PRS)
        requested = argv[argv.index("--json") + 1].split(",")
        self.assertIn("headRefName", requested)
        self.assertIn("isCrossRepository", requested)
        self.assertIn("changedFiles", requested)
        self.assertIn("files", requested)
        self.assertIn("number", requested)

    def test_a_pr_touching_no_files_yields_an_empty_list_not_a_crash(self):
        empty = {
            "pr": 9,
            "files": [],
            "headRefName": None,
            "isCrossRepository": None,
            "truncated": None,
            "file_count": 0,
        }
        with self._gh(json.dumps([{"number": 9, "files": []}])):
            self.assertEqual(mc.others_from_gh(), [empty])
        with self._gh(json.dumps([{"number": 9}])):
            self.assertEqual(mc.others_from_gh(), [empty])

    def test_a_truncated_files_list_is_recorded_not_raised_on_the_spot(self):
        """`gh` pages a PR's `files` at 100, and a short array is silent.

        Recorded rather than raised here, because this runs BEFORE the
        self-exclusion: raising would refuse on the caller's own large PR, the
        one entry the comparison is about to discard.
        """
        payload = json.dumps(
            [
                {
                    "number": 351,
                    "changedFiles": 120,
                    "files": [{"path": f"src/f{i}.rs"} for i in range(100)],
                }
            ]
        )
        with self._gh(payload):
            got = mc.others_from_gh()
        self.assertEqual(got[0]["truncated"], 120)
        self.assertEqual(got[0]["file_count"], 100)

    def test_a_truncated_pr_still_in_the_comparison_is_refused(self):
        others = [{"pr": 351, "files": [], "truncated": 120, "file_count": 100}]
        with self.assertRaises(mc.MigrationCollisionsError) as caught:
            mc.refuse_if_truncated(others)
        message = str(caught.exception)
        self.assertIn("truncated", message)
        self.assertIn("120", message)
        # The remedy must be actionable for a PR the caller does not own.
        self.assertIn("--others", message)

    def test_a_truncated_SELF_pr_does_not_block_its_own_enqueue(self):
        """The ordering the cross-check caught.

        The caller's own >100-file PR is discarded by `exclude_self`, so it must
        never reach the refusal — otherwise a large PR cannot enqueue at all and
        `--self-branch` could not rescue it, the raise having come first.
        """
        mine = {
            "pr": 427,
            "headRefName": "eng-1336",
            "files": [],
            "truncated": 120,
            "file_count": 100,
        }
        kept, dropped, _forks = mc.exclude_self([mine], "eng-1336")
        self.assertEqual(dropped, [427])
        # Must not raise.
        mc.refuse_if_truncated(kept)

    def test_a_complete_files_list_is_not_mistaken_for_a_truncated_one(self):
        payload = json.dumps(
            [
                {
                    "number": 351,
                    "changedFiles": 2,
                    "headRefName": "eng-351",
                    "files": [{"path": "a.sql"}, {"path": "b.rs"}],
                }
            ]
        )
        with self._gh(payload):
            got = mc.others_from_gh()
        self.assertEqual(got[0]["files"], ["a.sql", "b.rs"])

    def test_the_normalized_output_feeds_collisions(self):
        """The point of normalizing: the fetched shape must work with the same
        `collisions` the file path uses."""
        payload = json.dumps(
            [{"number": 351, "files": [{"path": "db-schema/migrations/0003_a.sql"}]}]
        )
        with self._gh(payload):
            others = mc.others_from_gh()
        found = mc.collisions(["db-schema/migrations/0003_b.sql"], others)
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0]["pr"], 351)

    def test_a_gh_failure_reports_its_last_line(self):
        """The LAST line, not the whole of stderr — gh prefixes real failures
        with progress noise, and dumping all of it is the payload this tool
        exists to avoid. Asserting only that the useful line is present passes
        equally against an implementation that dumps everything.
        """
        with self._gh("", returncode=1, stderr="noise\ngh: not authenticated"):
            with self.assertRaises(mc.MigrationCollisionsError) as ctx:
                mc.others_from_gh()
        message = str(ctx.exception)
        self.assertIn("not authenticated", message)
        self.assertNotIn("noise", message)

    def test_a_truncated_pr_list_is_refused_rather_than_reported_clear(self):
        """`gh pr list` truncates silently at `--limit`, and in a collision
        checker the dropped PR could be the colliding one — a "clear" verdict
        resting on an unknown. Exactly at the limit, truncation cannot be ruled
        out, so the tool refuses instead.
        """
        payload = json.dumps(
            [{"number": n, "files": [{"path": "a.sql"}]} for n in range(mc.GH_PR_LIMIT)]
        )
        with self._gh(payload):
            with self.assertRaises(mc.MigrationCollisionsError) as ctx:
                mc.others_from_gh()
        self.assertIn("may be truncated", str(ctx.exception))

    def test_a_list_below_the_limit_is_trusted(self):
        payload = json.dumps(
            [
                {"number": n, "files": [{"path": "a.sql"}]}
                for n in range(mc.GH_PR_LIMIT - 1)
            ]
        )
        with self._gh(payload):
            self.assertEqual(len(mc.others_from_gh()), mc.GH_PR_LIMIT - 1)

    def test_non_json_output_is_a_clean_error(self):
        with self._gh("not json"):
            with self.assertRaises(mc.MigrationCollisionsError):
                mc.others_from_gh()

    def test_the_two_sources_are_mutually_exclusive_and_one_is_required(self):
        for argv in (
            ["migration_collisions.py"],
            ["migration_collisions.py", "--others", "x.json", "--others-from-gh"],
        ):
            with self.subTest(argv=argv), self.assertRaises(SystemExit):
                with redirect_stderr(io.StringIO()):
                    mc.run(argv)


class ExcludeSelf(unittest.TestCase):
    """The fail-closed half of ENG-1336, reproduced offline.

    A pushed branch's own PR is in the open-PR listing, so the tool compared its
    migration against itself and exited non-zero on every run — refusing provably
    clean enqueues at least five times across three sessions in one day.
    """

    def _pr(self, number, branch, *files):
        return {"pr": number, "headRefName": branch, "files": list(files)}

    def test_the_callers_own_pr_is_dropped(self):
        others = [
            self._pr(427, "eng-1336", "db-schema/migrations/0014_mine.sql"),
            self._pr(351, "eng-351", "db-schema/migrations/0009_theirs.sql"),
        ]
        kept, dropped, _forks = mc.exclude_self(others, "eng-1336")
        self.assertEqual([e["pr"] for e in kept], [351])
        self.assertEqual(dropped, [427])

    def test_without_the_exclusion_a_branch_collides_with_itself(self):
        """The bug, stated as a test: this is what the gate used to see."""
        mine = ["db-schema/migrations/0014_mine.sql"]
        others = [self._pr(427, "eng-1336", "db-schema/migrations/0014_mine.sql")]
        self.assertEqual(len(mc.collisions(mine, others)), 1)
        kept, _, _forks = mc.exclude_self(others, "eng-1336")
        self.assertEqual(mc.collisions(mine, kept), [])

    def test_a_real_collision_survives_the_exclusion(self):
        """Excluding self must not become a way to miss the actual thing.

        Another PR claiming the same number is a different branch, so it stays in
        the set — which is why the match is on the branch and never on "their
        files look like mine".
        """
        mine = ["db-schema/migrations/0014_mine.sql"]
        others = [
            self._pr(427, "eng-1336", "db-schema/migrations/0014_mine.sql"),
            self._pr(430, "eng-999", "db-schema/migrations/0014_theirs.sql"),
        ]
        kept, dropped, _forks = mc.exclude_self(others, "eng-1336")
        found = mc.collisions(mine, kept)
        self.assertEqual(dropped, [427])
        self.assertEqual([c["pr"] for c in found], [430])

    def test_an_unresolved_branch_excludes_nothing(self):
        others = [self._pr(427, "eng-1336", "db-schema/migrations/0014_mine.sql")]
        kept, dropped, _forks = mc.exclude_self(others, None)
        self.assertEqual(kept, others)
        self.assertEqual(dropped, [])

    def test_an_entry_with_no_head_ref_is_kept(self):
        # A hand-assembled --others payload may omit it; that entry is simply
        # not self-excluded rather than crashing the compare.
        others = [{"pr": 42, "files": ["db-schema/migrations/0014_x.sql"]}]
        kept, dropped, _forks = mc.exclude_self(others, "eng-1336")
        self.assertEqual(kept, others)
        self.assertEqual(dropped, [])

    def test_a_fork_pr_sharing_the_branch_name_is_NOT_excluded(self):
        """The one way this exclusion could itself fail open.

        `gh` reports a head ref unqualified, so a fork's `eng-1336` compares
        equal to ours. Dropping it would remove a genuinely-colliding PR rather
        than the self-match. This repo is public, so the input is reachable.
        """
        ours = self._pr(427, "eng-1336", "db-schema/migrations/0014_mine.sql")
        theirs = dict(
            self._pr(999, "eng-1336", "db-schema/migrations/0014_theirs.sql"),
            isCrossRepository=True,
        )
        kept, dropped, _forks = mc.exclude_self([ours, theirs], "eng-1336")
        self.assertEqual(dropped, [427])
        self.assertEqual([e["pr"] for e in kept], [999])
        # And the collision it carries still fires.
        found = mc.collisions(["db-schema/migrations/0014_mine.sql"], kept)
        self.assertEqual([c["pr"] for c in found], [999])


class NextFreeNumber(unittest.TestCase):
    def test_it_is_one_past_the_highest_taken(self):
        self.assertEqual(mc.next_free_number({1, 2, 3, 13}), 14)

    def test_it_does_not_fill_a_gap(self):
        """One past the maximum, never the first gap.

        A gap may be held by something neither comparison set can see — a
        sibling merged since the merge-base is in neither — so only one past the
        maximum is unclaimed everywhere this tool looked.

        The input here is deliberately different from the case above: a
        first-gap implementation answers 2 for this set and 4 for that one, so
        reusing one input would leave the two tests unable to fail apart.
        """
        self.assertEqual(mc.next_free_number({1, 5}), 6)

    def test_an_empty_tree_starts_at_one(self):
        self.assertEqual(mc.next_free_number(set()), 1)


class AddedMigrations(unittest.TestCase):
    """``added_migrations`` over a real throwaway repo — the diff filter is the
    behavior under test, so git is not mocked."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = os.path.realpath(self._tmp.name)
        self._cwd = os.getcwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, self._cwd)
        self._git("init", "-q", "-b", "main")
        self._git("config", "user.email", "t@example.com")
        self._git("config", "user.name", "Test")
        os.makedirs(os.path.join(self.root, mc.DEFAULT_DIR))
        self._commit("0001_init.sql", "create table a();")
        self._git("checkout", "-q", "-b", "feature")

    def _git(self, *args: str) -> None:
        subprocess.run(["git", *args], cwd=self.root, check=True, capture_output=True)

    def _commit(self, name: str, body: str) -> None:
        rel = os.path.join(mc.DEFAULT_DIR, name)
        with open(os.path.join(self.root, rel), "w", encoding="utf-8") as fh:
            fh.write(body + "\n")
        self._git("add", rel)
        self._git("commit", "-q", "-m", f"add {name}", "--no-gpg-sign")

    def test_it_reports_only_this_branch_s_additions(self):
        self._commit("0002_new.sql", "create table b();")
        self.assertEqual(
            mc.added_migrations("main"),
            [os.path.join(mc.DEFAULT_DIR, "0002_new.sql")],
        )

    def test_an_edited_existing_migration_is_not_an_addition(self):
        # Editing an applied migration is a different (worse) problem -- it
        # breaks the recorded checksum -- and it is not a numbering collision,
        # so `--diff-filter=A` must not surface it as one.
        rel = os.path.join(mc.DEFAULT_DIR, "0001_init.sql")
        with open(os.path.join(self.root, rel), "a", encoding="utf-8") as fh:
            fh.write("-- touched\n")
        self._git("add", rel)
        self._git("commit", "-q", "-m", "edit", "--no-gpg-sign")
        self.assertEqual(mc.added_migrations("main"), [])

    def test_a_branch_touching_nothing_reports_nothing(self):
        self.assertEqual(mc.added_migrations("main"), [])

    def test_files_outside_the_migrations_directory_are_ignored(self):
        with open(os.path.join(self.root, "README.md"), "w", encoding="utf-8") as fh:
            fh.write("hi\n")
        self._git("add", "README.md")
        self._git("commit", "-q", "-m", "readme", "--no-gpg-sign")
        self.assertEqual(mc.added_migrations("main"), [])

    def test_the_cli_exits_non_zero_on_a_collision(self):
        self._commit("0002_new.sql", "create table b();")
        others = os.path.join(self.root, "others.json")
        with open(others, "w", encoding="utf-8") as fh:
            json.dump(
                [{"pr": 42, "files": [f"{mc.DEFAULT_DIR}/0002_theirs.sql"]}],
                fh,
            )
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = mc.run(
                ["migration_collisions.py", "--others", others, "--base", "main"]
            )
        self.assertEqual(code, 1)
        parsed = json.loads(out.getvalue())
        self.assertFalse(parsed["clear"])
        self.assertEqual(parsed["mine_numbers"], [2])
        self.assertIn("COLLISION", err.getvalue())

    def test_running_from_a_subdirectory_still_sees_this_branch_s_migrations(self):
        # `directory` is a git PATHSPEC, resolved against the cwd — while
        # DEFAULT_DIR is root-relative. Before the repo_root() pin, an off-root
        # run matched nothing and reported `clear: true`, exit 0, which is
        # indistinguishable from a genuinely clean branch on an enqueue gate.
        self._commit("0002_new.sql", "create table b();")
        sub = os.path.join(self.root, mc.DEFAULT_DIR)
        os.chdir(sub)
        self.addCleanup(os.chdir, self.root)
        self.assertEqual(
            mc.added_migrations("main"),
            [os.path.join(mc.DEFAULT_DIR, "0002_new.sql")],
        )

    def test_a_missing_migrations_directory_is_an_error_not_a_clear(self):
        # The other half of the same fail-open shape: a wrong --dir must not
        # answer "nothing collided".
        with self.assertRaises(mc.MigrationCollisionsError) as caught:
            mc.added_migrations("main", "no/such/dir")
        self.assertIn("refusing to report", str(caught.exception))

    def test_main_maps_a_bad_payload_to_exit_two_not_a_traceback(self):
        # The contract distinguishes 2 (bad input / git failure) from 1
        # (collision); a caller gating enqueue on the status depends on it.
        argv = [
            "migration_collisions.py",
            "--others",
            os.path.join(self.root, "nope.json"),
        ]
        err = io.StringIO()
        with mock.patch.object(mc.sys, "argv", argv):
            with redirect_stderr(err):
                code = mc.main()
        self.assertEqual(code, 2)
        self.assertIn("cannot read", err.getvalue())

    def test_main_maps_a_truncated_listing_to_exit_two_not_one(self):
        """Exit 2 is now a documented gate contract, so pin it through `main()`.

        The distinction is the whole point: 1 means "a collision", 2 means "could
        not answer". A caller that collapses them either reports a tool failure
        as a collision, or enqueues straight through an unanswered question.
        """
        others = os.path.join(self.root, "others.json")
        with open(others, "w", encoding="utf-8") as fh:
            json.dump(
                [
                    {
                        "pr": 351,
                        "files": [],
                        "truncated": 120,
                        "file_count": 100,
                    }
                ],
                fh,
            )
        argv = [
            "migration_collisions.py",
            "--others",
            others,
            "--base",
            "main",
        ]
        err = io.StringIO()
        with mock.patch.object(mc.sys, "argv", argv):
            with redirect_stderr(err):
                code = mc.main()
        self.assertEqual(code, 2)
        self.assertIn("truncated", err.getvalue())

    def test_the_cli_exits_zero_when_clear(self):
        self._commit("0002_new.sql", "create table b();")
        others = os.path.join(self.root, "others.json")
        with open(others, "w", encoding="utf-8") as fh:
            json.dump([{"pr": 42, "files": [f"{mc.DEFAULT_DIR}/0009_theirs.sql"]}], fh)
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = mc.run(
                ["migration_collisions.py", "--others", others, "--base", "main"]
            )
        self.assertEqual(code, 0)
        parsed = json.loads(out.getvalue())
        self.assertTrue(parsed["clear"])
        self.assertEqual(parsed["status"], "clear")
        self.assertIn("safe to enqueue", err.getvalue())

    def _run(self, others_payload, *extra):
        """Drive the CLI against this throwaway repo, returning (code, json)."""
        others = os.path.join(self.root, "others.json")
        with open(others, "w", encoding="utf-8") as fh:
            json.dump(others_payload, fh)
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = mc.run(
                [
                    "migration_collisions.py",
                    "--others",
                    others,
                    "--base",
                    "main",
                    *extra,
                ]
            )
        return code, json.loads(out.getvalue()), err.getvalue()

    def test_the_cli_exits_ZERO_on_the_self_pr_false_positive(self):
        """The regression that matters most: a clean tree must exit 0.

        The tool used to compare the branch's migration against its own PR and
        exit non-zero, which is exactly how a skill gates an enqueue — so it
        blocked every enqueue on a migration-carrying branch. Quieting the
        message would not have been enough; the exit status is the contract.
        """
        self._commit("0002_new.sql", "create table b();")
        # The branch's own PR, as `gh pr list` reports it once pushed.
        payload = [
            {
                "pr": 427,
                "headRefName": "feature",
                "files": [f"{mc.DEFAULT_DIR}/0002_new.sql"],
            }
        ]
        code, parsed, err = self._run(payload, "--self-branch", "feature")
        self.assertEqual(code, 0)
        self.assertEqual(parsed["status"], "clear")
        self.assertEqual(parsed["collisions"], [])
        self.assertEqual(parsed["self_prs"], [427])
        self.assertEqual(parsed["prs_checked"], 0)
        self.assertIn("safe to enqueue", err)

    def test_the_self_exclusion_resolves_the_branch_from_git_by_default(self):
        # setUp leaves the repo on `feature`, so no --self-branch is needed;
        # this is the shape the real --others-from-gh call takes.
        self._commit("0002_new.sql", "create table b();")
        payload = [
            {
                "pr": 427,
                "headRefName": "feature",
                "files": [f"{mc.DEFAULT_DIR}/0002_new.sql"],
            }
        ]
        code, parsed, _ = self._run(payload)
        self.assertEqual(code, 0)
        self.assertEqual(parsed["self_branch"], "feature")
        self.assertEqual(parsed["self_prs"], [427])

    def test_another_prs_collision_still_exits_non_zero(self):
        """The guard must still guard: self-exclusion is not a blanket pass."""
        self._commit("0002_new.sql", "create table b();")
        payload = [
            {
                "pr": 427,
                "headRefName": "feature",
                "files": [f"{mc.DEFAULT_DIR}/0002_new.sql"],
            },
            {
                "pr": 430,
                "headRefName": "eng-999",
                "files": [f"{mc.DEFAULT_DIR}/0002_theirs.sql"],
            },
        ]
        code, parsed, err = self._run(payload, "--self-branch", "feature")
        self.assertEqual(code, 1)
        self.assertEqual(parsed["status"], "collision")
        self.assertEqual([c["pr"] for c in parsed["collisions"]], [430])
        self.assertIn("COLLISION", err)

    def test_a_branch_with_no_migration_reports_nothing_claimed_and_the_number(self):
        """The fail-open half, end to end.

        `init-pr` calls the tool here on purpose — before the file exists — so
        the answer it needs is the number to take, not the word "clear".
        """
        payload = [
            {
                "pr": 430,
                "headRefName": "eng-999",
                "files": [f"{mc.DEFAULT_DIR}/0004_theirs.sql"],
            }
        ]
        code, parsed, err = self._run(payload)
        self.assertEqual(code, 0)
        self.assertEqual(parsed["status"], "nothing_claimed")
        self.assertEqual(parsed["mine"], [])
        # 0001 is in the tree and an open PR claims 0004, so the free number is
        # past both -- the open-PR set feeds the answer, not just the tree.
        self.assertEqual(parsed["next_free_number"], 5)
        self.assertIn("NOT a clear verdict", err)
        self.assertIn("0005", err)

    def test_a_detached_head_resolves_to_none_from_git(self):
        """The git → `None` link, which nothing else exercises.

        The two sibling tests *inject* `None`. If `current_branch()` regressed to
        returning the literal "HEAD", the self-match would come back — and it
        would come back **silently**, because the detached-HEAD note keys on
        `None` too, so the output would read like an ordinary run.
        """
        self.assertEqual(mc.current_branch(), "feature")
        self._git("checkout", "-q", "--detach")
        self.assertIsNone(mc.current_branch())

    def test_a_detached_head_run_reports_the_exclusion_was_not_applied(self):
        self._commit("0002_new.sql", "create table b();")
        self._git("checkout", "-q", "--detach")
        payload = [
            {
                "pr": 427,
                "headRefName": "feature",
                "files": [f"{mc.DEFAULT_DIR}/0002_new.sql"],
            }
        ]
        # `--base main` still resolves; only the branch name is gone.
        code, parsed, err = self._run(payload)
        self.assertIsNone(parsed["self_branch"])
        self.assertEqual(parsed["self_prs"], [])
        self.assertIn("detached HEAD", err)
        # Fail-closed: the self-match survives, so this still blocks.
        self.assertEqual(code, 1)

    def test_a_populated_self_branch_that_matches_nothing_says_so(self):
        """The silent case that would re-create the original misread.

        With the branch resolved but no listing entry matching it, the
        self-match survives into `collisions` and would otherwise print as an
        ordinary COLLISION with no hint.
        """
        self._commit("0002_new.sql", "create table b();")
        payload = [
            {
                "pr": 427,
                "headRefName": "some-other-head-ref",
                "files": [f"{mc.DEFAULT_DIR}/0002_new.sql"],
            }
        ]
        code, parsed, err = self._run(payload)
        self.assertEqual(code, 1)
        self.assertEqual(parsed["self_prs"], [])
        self.assertIn("no PR was excluded", err)

    def test_an_explicit_self_branch_override_is_visible_in_the_summary(self):
        # A bypass must never be silent: the flag can turn a collision into a
        # clear verdict, so the summary records that it was used.
        self._commit("0002_new.sql", "create table b();")
        payload = [
            {
                "pr": 427,
                "headRefName": "feature",
                "files": [f"{mc.DEFAULT_DIR}/0002_new.sql"],
            }
        ]
        _, parsed, err = self._run(payload, "--self-branch", "feature")
        self.assertTrue(parsed["self_branch_explicit"])
        self.assertIn("--self-branch", err)

    def test_an_empty_self_branch_does_not_fall_back_to_git(self):
        # `or` would have read "" as unset and silently used the ambient branch,
        # making an offline --others compare worktree-dependent.
        self._commit("0002_new.sql", "create table b();")
        payload = [
            {
                "pr": 427,
                "headRefName": "feature",
                "files": [f"{mc.DEFAULT_DIR}/0002_new.sql"],
            }
        ]
        code, parsed, _ = self._run(payload, "--self-branch", "")
        self.assertEqual(parsed["self_branch"], "")
        self.assertEqual(parsed["self_prs"], [])
        # Nothing excluded, so the self-match stands and this blocks.
        self.assertEqual(code, 1)

    def test_nothing_claimed_still_reports_clear_true_for_compatibility(self):
        # `clear` is deliberately retained with its original meaning. Pinning the
        # pair stops a well-meant "fix" to `clear` from changing it silently.
        code, parsed, _ = self._run([])
        self.assertEqual(code, 0)
        self.assertEqual(parsed["status"], "nothing_claimed")
        self.assertTrue(parsed["clear"])

    def test_an_uncommitted_migration_still_counts_as_taken(self):
        # `tree_numbers` reads the directory rather than git, so a number this
        # branch has written but not committed cannot be handed out again.
        with open(
            os.path.join(self.root, mc.DEFAULT_DIR, "0007_wip.sql"),
            "w",
            encoding="utf-8",
        ) as fh:
            fh.write("-- wip\n")
        self.assertEqual(mc.tree_numbers(), [1, 7])
        code, parsed, _ = self._run([])
        self.assertEqual(code, 0)
        self.assertEqual(parsed["next_free_number"], 8)


if __name__ == "__main__":
    unittest.main()
