#!/usr/bin/env python3
"""Unit tests for `iterm_deploy.py` (stdlib unittest).

Everything runs against temp directories standing in for the base checkout,
`~/.claude/scripts` and iTerm's `AutoLaunch/`, so no test touches the
operator's live integration.
"""

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

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


class CheckMode(DeployHarness):
    def test_check_exits_1_until_linked_and_writes_nothing(self):
        self.scripts.mkdir(parents=True)
        copy = self.scripts / "iterm-paint.sh"
        copy.write_text("july copy", encoding="utf-8")
        argv = [
            "--source",
            str(self.source),
            "--scripts-dir",
            str(self.scripts),
            "--autolaunch-dir",
            str(self.autolaunch),
        ]
        with open(os.devnull, "w", encoding="utf-8") as sink:
            stdout, iterm_deploy.sys.stdout = iterm_deploy.sys.stdout, sink
            try:
                self.assertEqual(iterm_deploy.main(argv), 1)
            finally:
                iterm_deploy.sys.stdout = stdout
        self.assertFalse(copy.is_symlink(), "a dry run must not touch anything")


if __name__ == "__main__":
    unittest.main()
