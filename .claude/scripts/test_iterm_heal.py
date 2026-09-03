#!/usr/bin/env python3
"""Behavior tests for the iTerm permission-yellow heal (stdlib unittest).

The predicate under test is **shell**, and it stays shell: the monitor calls it
every ~3 seconds per tab, and that file already avoids process spawns at poll
cadence on purpose (`stat` and `date` are cheap C binaries; a `python3` start is
not). So rather than port the arithmetic to Python and have the shell call it —
which would undo that decision — these tests drive the real `iterm-colors.sh`
in a subprocess.

That also means the coverage lands in `make tools-tests`, which already
discovers `test_*.py` under `.claude/scripts`, instead of needing a shell test
runner the repo does not have. Landing it here is what retires the throwaway
`heal_test.sh` shape, and with it the `zsh <script>` permission churn that is
structurally unfirmable (`Bash(zsh:*)` is a bare-verb wildcard the safety floor
in `firm_core.is_bareverb_wildcard` refuses outright).
"""

from __future__ import annotations

import os
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

COLORS = Path(__file__).resolve().parent / "iterm-colors.sh"

# Kept in step with iterm-colors.sh deliberately: a test that read the palette
# out of the file under test could not catch the palette changing.
STATE_PERMISSION = "3a2c08"
STATE_NEUTRAL = "16191e"
STATE_REPLY = "080c2a"


class HealHarness(unittest.TestCase):
    """Each case builds a per-tty state/sentinel pair under a temp STATE_PREFIX
    and asks the real shell function what it does with them."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.prefix = os.path.join(self._tmp.name, "iterm-color-")
        self.tty = "ttys999"

    def _state_file(self):
        return Path(f"{self.prefix}{self.tty}")

    def _sentinel(self):
        return Path(f"{self.prefix}{self.tty}.permwait")

    def _heal(self, *, stale_seconds="120"):
        """Run `heal_stale_permission` against the fixture, return its output."""
        script = (
            f'STATE_PREFIX="{self.prefix}"; '
            f'source "{COLORS}"; '
            f'STATE_PREFIX="{self.prefix}"; '
            f'PERM_WAIT_STALE_SECONDS="{stale_seconds}"; '
            f'heal_stale_permission "{self.tty}"'
        )
        return subprocess.run(
            ["bash", "-c", script],
            capture_output=True,
            text=True,
            check=False,
        )

    def _age_sentinel(self, seconds):
        stamp = time.time() - seconds
        os.utime(self._sentinel(), (stamp, stamp))


class HealsAStalePermission(HealHarness):
    def test_a_stale_sentinel_on_a_yellow_tab_heals_to_neutral(self):
        """The measured wedge: approve, a guard denies, the tool errors red, and
        nothing on the denial path ever repaints — so the tab stays yellow over
        a session that wants nothing."""
        self._state_file().write_text(STATE_PERMISSION, encoding="utf-8")
        self._sentinel().write_text("", encoding="utf-8")
        self._age_sentinel(300)

        result = self._heal()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self._state_file().read_text(encoding="utf-8").strip(), STATE_NEUTRAL
        )
        self.assertFalse(self._sentinel().exists(), "the sentinel must be cleared")

    def test_the_threshold_boundary_heals_at_exactly_the_limit(self):
        """`-ge`, not `-gt` — pinned because the boundary is the one value a
        future rewrite is most likely to move by one."""
        self._state_file().write_text(STATE_PERMISSION, encoding="utf-8")
        self._sentinel().write_text("", encoding="utf-8")
        self._age_sentinel(120)
        self._heal(stale_seconds="120")
        self.assertEqual(
            self._state_file().read_text(encoding="utf-8").strip(), STATE_NEUTRAL
        )

    def test_a_custom_threshold_is_honored(self):
        self._state_file().write_text(STATE_PERMISSION, encoding="utf-8")
        self._sentinel().write_text("", encoding="utf-8")
        self._age_sentinel(30)
        self._heal(stale_seconds="10")
        self.assertEqual(
            self._state_file().read_text(encoding="utf-8").strip(), STATE_NEUTRAL
        )


class DoesNotHeal(HealHarness):
    """The four non-healing guards. Each is a case where healing would be the
    bug, and the failure direction is asymmetric: dropping the yellow on a
    prompt that really is waiting can stall a session, which is strictly worse
    than a lingering tint."""

    def test_a_fresh_sentinel_is_left_alone(self):
        """The harness re-fires permission_prompt while a prompt waits, so a
        fresh sentinel means it is still waiting."""
        self._state_file().write_text(STATE_PERMISSION, encoding="utf-8")
        self._sentinel().write_text("", encoding="utf-8")
        self._age_sentinel(5)
        self._heal()
        self.assertEqual(
            self._state_file().read_text(encoding="utf-8").strip(), STATE_PERMISSION
        )
        self.assertTrue(self._sentinel().exists())

    def test_an_edit_tool_yellow_with_no_sentinel_is_left_alone(self):
        """An edit tool's PreToolUse also paints yellow but carries no
        sentinel, and its lifetime is not governed by permission re-fires — so
        it must never be healed."""
        self._state_file().write_text(STATE_PERMISSION, encoding="utf-8")
        self._heal()
        self.assertEqual(
            self._state_file().read_text(encoding="utf-8").strip(), STATE_PERMISSION
        )

    def test_a_green_tab_is_not_repainted(self):
        """A stale sentinel plus a non-yellow state is not a lingering
        permission — repainting here would clobber a reply-wanted green."""
        self._state_file().write_text(STATE_REPLY, encoding="utf-8")
        self._sentinel().write_text("", encoding="utf-8")
        self._age_sentinel(300)
        self._heal()
        self.assertEqual(
            self._state_file().read_text(encoding="utf-8").strip(), STATE_REPLY
        )
        self.assertTrue(self._sentinel().exists())

    def test_a_missing_state_file_is_not_created(self):
        self._sentinel().write_text("", encoding="utf-8")
        self._age_sentinel(300)
        self._heal()
        self.assertFalse(self._state_file().exists())

    def test_a_blank_tty_base_is_a_no_op(self):
        """Guards against a caller passing an unset variable, which would
        otherwise operate on the bare STATE_PREFIX path."""
        script = (
            f'STATE_PREFIX="{self.prefix}"; '
            f'source "{COLORS}"; '
            f'STATE_PREFIX="{self.prefix}"; '
            'heal_stale_permission ""'
        )
        result = subprocess.run(
            ["bash", "-c", script], capture_output=True, text=True, check=False
        )
        self.assertEqual(result.returncode, 0, result.stderr)


class ThresholdDefault(unittest.TestCase):
    def test_the_default_is_120_and_is_overridable(self):
        """`PERM_WAIT_STALE_SECONDS` is the one behavioral constant in this
        integration with no measurement behind it. Its correctness rests on the
        harness re-firing permission_prompt MORE OFTEN than the threshold,
        which has never been measured — so this pins the value and its override
        rather than claiming the value is right."""
        for env, want in (({}, "120"), ({"ITERM_PERM_WAIT_STALE_SECONDS": "45"}, "45")):
            with self.subTest(env=env):
                result = subprocess.run(
                    [
                        "bash",
                        "-c",
                        f'source "{COLORS}"; printf "%s" "$PERM_WAIT_STALE_SECONDS"',
                    ],
                    capture_output=True,
                    text=True,
                    check=False,
                    env={**os.environ, **env},
                )
                self.assertEqual(result.stdout.strip(), want)


if __name__ == "__main__":
    unittest.main()
