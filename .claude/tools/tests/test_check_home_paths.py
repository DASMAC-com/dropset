#!/usr/bin/env python3
# cspell:word jdoe
# cspell:word asmith
"""Unit tests for ``check_home_paths.py`` (stdlib ``unittest``; no pytest).

The load-bearing property is the **asymmetry**: a placeholder user segment
passes and a real-looking one fails. A guard that rejected every home-shaped
path would fail half this directory's own fixtures, so each case below pins one
side of that line.

**Every offending fixture is assembled at run time** — see ``mac`` / ``linux``
below. A literal offending path written out in this file would trip the very
guard under test, since this module sits inside the tree the hook is wired over.
Assembling it is preferable to giving the guard an escape hatch or excluding
this path from the hook: both would be a hole in a check whose whole value is
having none.
"""

from __future__ import annotations

import io
import os
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr

import check_home_paths as chp

# Account names that must be rejected. Names, not paths — the prefix is joined
# on below so no offending path literal exists in this file.
REAL = "jdoe"
OTHER = "asmith"


def mac(name: str, tail: str = "") -> str:
    """A macOS home path for ``name``, built rather than written."""
    return "/" + "Users" + "/" + name + tail


def linux(name: str, tail: str = "") -> str:
    """A Linux home path for ``name``, built rather than written."""
    return "/" + "home" + "/" + name + tail


class OffendingSegments(unittest.TestCase):
    def test_a_real_looking_account_is_flagged(self):
        line = f"base = '{mac(REAL, '/repos/dropset')}'"
        self.assertEqual(chp.offending_segments(line), [REAL])

    def test_each_placeholder_passes(self):
        # The sibling tools' fixtures depend on every one of these.
        for name in sorted(chp.PLACEHOLDERS):
            with self.subTest(placeholder=name):
                self.assertEqual(chp.offending_segments(mac(name, "/**")), [])

    def test_linux_home_prefix_is_covered_too(self):
        self.assertEqual(chp.offending_segments(linux(REAL, "/.cargo/bin")), [REAL])

    def test_a_placeholder_does_not_excuse_a_real_path_later_on_the_line(self):
        line = f"cp {mac('me', '/a')} {mac(REAL, '/b')}"
        self.assertEqual(chp.offending_segments(line), [REAL])

    def test_two_offenders_on_one_line_are_both_reported(self):
        line = f"{mac(REAL, '/x')} and {linux(OTHER, '/y')}"
        self.assertEqual(chp.offending_segments(line), [REAL, OTHER])

    def test_an_unrelated_absolute_path_is_not_a_home_path(self):
        self.assertEqual(chp.offending_segments("/var/folders/6_/tmp"), [])
        self.assertEqual(chp.offending_segments("/repos/dropset/.claude"), [])

    def test_the_bare_prefix_with_no_segment_is_not_flagged(self):
        self.assertEqual(chp.offending_segments("under " + mac("") + " here"), [])


class ScanText(unittest.TestCase):
    def test_reports_the_line_number_and_segment(self):
        text = "clean\n" + f"base = '{mac(REAL, '/repos/dropset')}'\n" + "clean\n"
        findings = chp.scan_text("t.py", text)
        self.assertEqual(len(findings), 1)
        self.assertEqual(findings[0].path, "t.py")
        self.assertEqual(findings[0].line_no, 2)
        self.assertEqual(findings[0].segment, REAL)
        self.assertIn(REAL, findings[0].render())

    def test_a_clean_file_yields_nothing(self):
        self.assertEqual(chp.scan_text("t.py", mac("me", "/**") + "\n"), [])


class ScanFiles(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = self._tmp.name

    def _write(self, rel: str, body: bytes) -> str:
        path = os.path.join(self.root, rel)
        with open(path, "wb") as fh:
            fh.write(body)
        return path

    def test_binary_files_are_skipped_not_fatal(self):
        # A committed .wasm under a scanned tree must not fail the hook for a
        # reason unrelated to the rule.
        blob = b"\x00\x01\xff\xfe" + mac(REAL, "/x").encode("utf-8")
        self.assertEqual(chp.scan_files([self._write("blob.wasm", blob)]), [])

    def test_a_missing_path_is_skipped_not_fatal(self):
        self.assertEqual(chp.scan_files([os.path.join(self.root, "gone.py")]), [])

    def test_main_exits_non_zero_and_names_the_file(self):
        body = f"HOME = '{mac(REAL, '/repos/dropset')}'\n".encode("utf-8")
        bad = self._write("bad.py", body)
        with redirect_stderr(io.StringIO()) as err:
            self.assertEqual(chp.main([bad]), 1)
        message = err.getvalue()
        self.assertIn("bad.py", message)
        self.assertIn(REAL, message)
        self.assertIn("placeholder", message)

    def test_main_exits_zero_on_a_clean_set(self):
        body = f"HOME = '{mac('me', '/repos/dropset')}'\n".encode("utf-8")
        self.assertEqual(chp.main([self._write("good.py", body)]), 0)

    def test_no_paths_is_a_pass(self):
        # pre-commit invokes the hook with an empty list when nothing in the
        # commit matches `files:`.
        self.assertEqual(chp.main([]), 0)


class TheRepoItselfIsClean(unittest.TestCase):
    """The guard must pass on the trees it is wired over — otherwise it lands
    red and gets disabled rather than obeyed. This is the same set the hook's
    ``files:`` regex selects, checked here so a violation surfaces in the suite
    and not only at lint time.

    **The file list comes from git, and that is load-bearing twice over.**

    It fixes the scope: the hook is fed *tracked* files by pre-commit, so a
    tracked-only list is what actually mirrors it. Walking the tree instead
    would additionally pick up the **gitignored, machine-local**
    ``.claude/settings.local.json``, which legitimately carries absolute home
    paths — the same carve-out ``allowlist.is_machine_local_settings`` encodes
    — and this test would land red on the operator's machine for a file the
    hook never sees.

    And it fixes the root: an earlier version derived the root by walking
    ``os.pardir`` twice from this file, which lands on ``<repo>/.claude`` and
    made the whole assertion **vacuous** — it walked two directories that do
    not exist, scanned zero files, and asserted ``[] == []``. It would have
    passed with every file in the repo naming a real home directory. Hence the
    non-zero count assertion below: a check that cannot report what it examined
    cannot be trusted to have examined anything.
    """

    # Mirrors the hook's `files:` regex in cfg/pre-commit-lint.yml. Keep in
    # sync with it: this test is the tripwire for that hook, not a separate
    # policy.
    SCOPES = (".claude/", "docs/conventions/", "CLAUDE.md")

    def _tracked(self) -> list[str]:
        root = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
        listing = subprocess.run(
            ["git", "ls-files", "-z", "--", *self.SCOPES],
            capture_output=True,
            text=True,
            check=True,
            cwd=root,
        ).stdout
        return [
            os.path.join(root, p)
            for p in listing.split("\0")
            if p and os.path.lexists(os.path.join(root, p))
        ]

    def test_committed_agent_material_has_no_real_home_paths(self):
        scanned = self._tracked()
        # The anti-vacuity assertion. Without it this test passed while
        # scanning nothing at all.
        self.assertGreater(
            len(scanned),
            50,
            "resolved suspiciously few files — the scope is probably wrong, "
            "and a scan of nothing trivially reports clean",
        )
        findings = chp.scan_files(scanned)
        self.assertEqual(
            [f.render() for f in findings],
            [],
            "committed agent material names a real home directory",
        )

    def test_the_scan_would_actually_catch_a_violation_in_that_scope(self):
        """Proves the tripwire is armed, not merely silent.

        The case above can only ever report "nothing found", which is exactly
        the shape that hid its own vacuity. This one confirms the same
        machinery flags a planted violation.
        """
        planted = self._tracked()[:1]
        self.assertTrue(planted, "no tracked agent-material files resolved")
        findings = chp.scan_text(planted[0], f"home = '{mac(REAL, '/x')}'")
        self.assertEqual(len(findings), 1)
        self.assertEqual(findings[0].segment, REAL)


if __name__ == "__main__":
    unittest.main()
