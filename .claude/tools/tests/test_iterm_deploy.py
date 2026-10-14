#!/usr/bin/env python3
"""Unit tests for `iterm_deploy.py` (stdlib unittest).

Everything runs against temp directories standing in for the base checkout,
`~/.claude/scripts` and iTerm's `AutoLaunch/`, so no test touches the
operator's live integration.
"""

from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import iterm_deploy


class DeployHarness(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        root = Path(self._tmp.name)
        self.source = root / "repo" / ".claude" / "scripts"
        self.source.mkdir(parents=True)
        for name in ("iterm-paint.sh", "iterm-colors.sh", iterm_deploy.REORDERER):
            (self.source / name).write_text(f"# {name}\n", encoding="utf-8")
        # Not part of the integration, so never deployed.
        (self.source / "test_iterm_heal.py").write_text("", encoding="utf-8")
        self.scripts = root / "home" / ".claude" / "scripts"
        self.autolaunch = root / "AutoLaunch"

    def pairs(self):
        return iterm_deploy.targets(self.source, self.scripts, self.autolaunch)


class Targets(DeployHarness):
    def test_every_committed_iterm_script_and_no_test_file(self):
        names = sorted(link.name for link, _ in self.pairs())
        self.assertEqual(
            names, ["iterm-colors.sh", "iterm-paint.sh", iterm_deploy.REORDERER]
        )

    def test_autolaunch_is_included_only_where_it_was_set_up(self):
        self.autolaunch.mkdir()
        links = [link for link, _ in self.pairs()]
        self.assertIn(self.autolaunch / iterm_deploy.REORDERER, links)


class Status(DeployHarness):
    def test_the_four_states(self):
        self.scripts.mkdir(parents=True)
        src = self.source / "iterm-paint.sh"
        link = self.scripts / "iterm-paint.sh"
        self.assertEqual(iterm_deploy.status(link, src), "missing")
        link.write_text("old copy", encoding="utf-8")
        self.assertEqual(iterm_deploy.status(link, src), "stale-copy")
        link.unlink()
        link.symlink_to(self.source / "iterm-colors.sh")
        self.assertEqual(iterm_deploy.status(link, src), "wrong-link")
        link.unlink()
        link.symlink_to(src)
        self.assertEqual(iterm_deploy.status(link, src), "linked")


class Apply(DeployHarness):
    def test_a_copy_is_backed_up_never_deleted_and_replaced_by_a_link(self):
        """The measured failure: a hand-made copy months behind main."""
        self.scripts.mkdir(parents=True)
        (self.scripts / "iterm-paint.sh").write_text("july copy", encoding="utf-8")
        backups = iterm_deploy.apply(self.pairs(), "stamp")
        self.assertEqual(len(backups), 1)
        self.assertEqual(Path(backups[0]).read_text(encoding="utf-8"), "july copy")
        for link, src in self.pairs():
            self.assertEqual(iterm_deploy.status(link, src), "linked", link)

    def test_an_edit_to_the_source_is_live_through_the_link(self):
        """The point of the whole change: committed code IS the running code."""
        iterm_deploy.apply(self.pairs(), "stamp")
        (self.source / "iterm-paint.sh").write_text("new code", encoding="utf-8")
        self.assertEqual(
            (self.scripts / "iterm-paint.sh").read_text(encoding="utf-8"), "new code"
        )

    def test_a_wrong_link_is_replaced_without_a_backup(self):
        self.scripts.mkdir(parents=True)
        stray = self.scripts / "iterm-paint.sh"
        stray.symlink_to(self.source / "iterm-colors.sh")
        self.assertEqual(iterm_deploy.apply(self.pairs(), "stamp"), [])
        self.assertEqual(stray.resolve(), (self.source / "iterm-paint.sh").resolve())

    def test_a_second_apply_is_a_no_op(self):
        iterm_deploy.apply(self.pairs(), "stamp")
        self.assertEqual(iterm_deploy.apply(self.pairs(), "stamp2"), [])


class Orphans(DeployHarness):
    def test_an_older_family_is_reported_not_touched(self):
        self.scripts.mkdir(parents=True)
        old = self.scripts / "iterm-bg-old.sh"
        old.write_text("", encoding="utf-8")
        self.assertEqual(
            iterm_deploy.orphans(self.source, self.scripts), ["iterm-bg-old.sh"]
        )
        self.assertTrue(old.exists())


class IsSource(DeployHarness):
    def test_a_deploy_dir_that_is_the_checkout_is_never_backed_up(self):
        """The data-loss case: with the deploy dir itself a link into the
        checkout, every target IS a committed file, and "backing it up" would
        move it out of the repo and leave a link pointing at itself."""
        self.scripts.parent.mkdir(parents=True)
        self.scripts.symlink_to(self.source)
        pairs = self.pairs()
        self.assertEqual({iterm_deploy.status(*p) for p in pairs}, {"is-source"})
        self.assertEqual(iterm_deploy.apply(pairs, "stamp"), [])
        self.assertEqual(
            (self.source / "iterm-paint.sh").read_text(encoding="utf-8"),
            "# iterm-paint.sh\n",
        )
        self.assertFalse((self.source / "iterm-paint.sh").is_symlink())


class Main(DeployHarness):
    def run_main(self, *extra):
        argv = [
            "--source",
            str(self.source),
            "--scripts-dir",
            str(self.scripts),
            "--autolaunch-dir",
            str(self.autolaunch),
            *extra,
        ]
        self.calls = []

        def fake_run(cmd, **_kwargs):
            self.calls.append(cmd)
            return subprocess.CompletedProcess(cmd, 0, stdout="stopped 0", stderr="")

        with (
            open(os.devnull, "w", encoding="utf-8") as sink,
            mock.patch.object(iterm_deploy.subprocess, "run", fake_run),
            mock.patch.object(iterm_deploy.sys, "stdout", sink),
        ):
            return iterm_deploy.main(argv)

    def kill_calls(self):
        return [c for c in self.calls if c[0] == "pkill"]

    def test_check_exits_1_until_linked_and_writes_nothing(self):
        self.scripts.mkdir(parents=True)
        copy = self.scripts / "iterm-paint.sh"
        copy.write_text("july copy", encoding="utf-8")
        self.assertEqual(self.run_main(), 1)
        self.assertFalse(copy.is_symlink(), "a dry run must not touch anything")
        self.assertEqual(self.calls, [], "a dry run must stop nothing")

    def test_an_empty_source_is_an_error_not_a_vacuous_pass(self):
        self.source = self.source.parent / "empty"
        self.source.mkdir()
        self.assertEqual(self.run_main(), 2)

    def test_apply_stops_the_reorderer_only_where_autolaunch_exists(self):
        self.assertEqual(self.run_main("--apply"), 0)
        self.assertEqual(self.kill_calls(), [])

    def test_apply_stops_the_reorderer_by_its_autolaunch_path(self):
        # Anchored, so an editor or pager holding the file is spared.
        self.autolaunch.mkdir()
        self.run_main("--apply")
        self.assertEqual(
            self.kill_calls(), [["pkill", "-f", "AutoLaunch/iterm-reorder[.]py"]]
        )

    def test_a_repeat_apply_on_a_linked_tree_stops_nothing(self):
        self.autolaunch.mkdir()
        self.run_main("--apply")
        self.assertEqual(self.run_main("--apply"), 0)
        self.assertEqual(self.calls, [])


if __name__ == "__main__":
    unittest.main()
