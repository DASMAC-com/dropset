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
    def _result(self, mine, numbers, found, prs=2):
        return {
            "mine": mine,
            "mine_numbers": numbers,
            "prs_checked": prs,
            "collisions": found,
            "clear": not found,
        }

    def test_no_migration_says_so(self):
        line = mc.summarize(self._result([], [], []))
        self.assertIn("adds no migration", line)

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
                }
            ],
        )

    def test_a_pr_touching_no_files_yields_an_empty_list_not_a_crash(self):
        with self._gh(json.dumps([{"number": 9, "files": []}])):
            self.assertEqual(mc.others_from_gh(), [{"pr": 9, "files": []}])
        with self._gh(json.dumps([{"number": 9}])):
            self.assertEqual(mc.others_from_gh(), [{"pr": 9, "files": []}])

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
        self.assertTrue(json.loads(out.getvalue())["clear"])
        self.assertIn("safe to enqueue", err.getvalue())


if __name__ == "__main__":
    unittest.main()
