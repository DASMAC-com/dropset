"""Run every guard hook's ``--self-test`` from the discovered suite.

The four `PreToolUse` guards are the most security-sensitive parsers in
`.claude/`, and they were also the least covered by anything automated. Each
carries a substantial in-file case table — together over 150 cases — but
`make tools-tests` discovers `.claude/tools/tests` and `.claude/scripts` and
**not** `.claude/hooks`, so the whole lot ran only when a human happened to type
`--self-test`. Two independent adversarial passes over the same diff flagged
that gap, having each just found a real defect in one of those parsers: a
regression that turned a read-only `rg "rm -rf $HOME"` into an un-overridable
deny, and a `..` spelling that walked straight through the worktree guard. Both
were caught by review rather than by a suite, which is the argument for this
file.

Shelling out rather than importing is deliberate. A hook is invoked by the
harness as a script, `--self-test` is its real entry point, and a subprocess
exercises argument parsing and the exit code the harness actually reads. It also
keeps this file indifferent to the hooks' module-level names, so it does not
break when one of them refactors.

Nothing here is platform-gated: the guards are pure-Python string parsers, so
unlike the two shell-driving suites under `.claude/scripts` these cases run on
the Linux CI runner exactly as they do locally.
"""

from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path

HOOKS = Path(__file__).resolve().parents[2] / "hooks"

# Explicit, not globbed. A glob would silently shrink to nothing if the
# directory moved, and report success while testing zero hooks — the same
# vacuous-pass shape `convention_refs.py` refuses a zero count for. The
# companion scan below catches the opposite drift: a guard that grows a
# self-test and is not listed here.
EXPECTED = (
    "no_compound_bash.py",
    "no_destructive_bash.py",
    "no_git_grep.py",
    "worktree_edit_guard.py",
)


class HookSelfTests(unittest.TestCase):
    def test_every_expected_hook_is_present(self):
        """A renamed or deleted guard must fail loudly, not vanish quietly."""
        missing = [name for name in EXPECTED if not (HOOKS / name).is_file()]
        self.assertEqual(missing, [], f"guard scripts missing from {HOOKS}")

    def test_each_self_test_passes(self):
        for name in EXPECTED:
            with self.subTest(hook=name):
                result = subprocess.run(
                    [sys.executable, str(HOOKS / name), "--self-test"],
                    capture_output=True,
                    text=True,
                    timeout=60,
                )
                self.assertEqual(
                    result.returncode,
                    0,
                    f"{name} --self-test failed:\n{result.stdout}\n{result.stderr}",
                )

    def test_each_self_test_reports_a_nonzero_case_count(self):
        """Exit 0 alone is not evidence: a self-test whose case table is empty,
        or whose loop never runs, also exits 0. Each hook prints its own count,
        so require a number in the output and require it to be positive.
        """
        for name in EXPECTED:
            with self.subTest(hook=name):
                result = subprocess.run(
                    [sys.executable, str(HOOKS / name), "--self-test"],
                    capture_output=True,
                    text=True,
                    timeout=60,
                )
                digits = [
                    int(token)
                    for token in result.stdout.replace("(", " ").split()
                    if token.isdigit()
                ]
                self.assertTrue(
                    digits and max(digits) > 0,
                    f"{name} --self-test printed no case count: {result.stdout!r}",
                )

    def test_no_unlisted_hook_has_a_self_test(self):
        """The reverse drift: a guard that grows a self-test must be added to
        ``EXPECTED`` rather than being covered by nobody.
        """
        unlisted = sorted(
            path.name
            for path in HOOKS.glob("*.py")
            if path.name not in EXPECTED
            and "--self-test" in path.read_text(encoding="utf-8")
        )
        self.assertEqual(
            unlisted, [], "these hooks expose --self-test but are not in EXPECTED"
        )


if __name__ == "__main__":
    unittest.main()
