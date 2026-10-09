"""The `plan` launcher's cycle loop, run for real with the client stubbed.

A cycled planning session leaves `plan-cycle-pending` and ends its client; the
loop must relaunch in place under a fresh id and the same name, bump the day's
counter, and refuse when a live session already holds the name. Each case runs
the real `plan` with `_ds_session` replaced by a recorder.

Run with: ``python3 -m unittest discover -s .claude/scripts``
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
import unittest
from datetime import datetime
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
INIT = REPO / ".claude" / "shell" / "init.zsh"
ZSH = shutil.which("zsh")
_NEEDS_ZSH = "the session helpers are zsh; no zsh on this machine"

#: Everything `plan` touches before the launch, stubbed so a case tests only the
#: loop. The recorder writes the marker for the first `$CYCLES` launches, the
#: way a cycling session's `plan_cycle.py cycle` would before its client exits.
_SCRIPT = """
source "$INIT" 2>/dev/null
_DS_REPO="$REPO" _DS_SUBSTRATE_DIR="$STATE"
_ds_tier() { print -r -- model; print -r -- anthropic }
_ds_substrate_enter() { : }
_ds_aws_login() { : }
_ds_session() {
  print -r -- "$1|$2|$3" >> "$CALLS"
  local n=$(wc -l < "$CALLS")
  (( n <= CYCLES )) && print -r -- "$1" > "$STATE/plan-cycle-pending"
  return 0
}
plan
"""


@unittest.skipUnless(ZSH, _NEEDS_ZSH)
class CycleLoop(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.state = self.root / "state"
        self.state.mkdir()
        self.calls = self.root / "calls"
        self.calls.touch()
        self.home = self.root / "claude"
        (self.home / "sessions").mkdir(parents=True)
        self.day = datetime.now().strftime("%Y%m%d")
        self.name = f"plan-{datetime.now().day}"

    def _plan(self, cycles: int) -> subprocess.CompletedProcess:
        env = {
            **os.environ,
            "INIT": str(INIT),
            "REPO": str(REPO),
            "STATE": str(self.state),
            "CALLS": str(self.calls),
            "CYCLES": str(cycles),
            "CLAUDE_CONFIG_DIR": str(self.home),
        }
        return subprocess.run(
            [ZSH, "-c", _SCRIPT], capture_output=True, text=True, env=env, check=False
        )

    def _launches(self) -> list[list[str]]:
        return [line.split("|") for line in self.calls.read_text().splitlines()]

    def test_no_marker_means_one_launch(self):
        self.assertEqual(self._plan(cycles=0).returncode, 0)
        self.assertEqual(len(self._launches()), 1)
        self.assertFalse((self.state / f"plan-cycle-{self.day}").exists())

    def test_a_cycle_relaunches_under_a_fresh_id_and_the_same_name(self):
        result = self._plan(cycles=1)
        self.assertEqual(result.returncode, 0, result.stderr)
        (sid0, name0, prompt0), (sid1, name1, prompt1) = self._launches()
        self.assertNotEqual(sid0, sid1)
        self.assertEqual((name0, name1), (self.name, self.name))
        self.assertEqual((prompt0, prompt1), ("/plan", "/plan"))
        counter = self.state / f"plan-cycle-{self.day}"
        self.assertEqual(counter.read_text().strip(), "1")
        self.assertFalse((self.state / "plan-cycle-pending").exists())

    def test_a_later_reopen_resumes_the_latest_cycle(self):
        self._plan(cycles=1)
        cycled = self._launches()[-1][0]
        self.calls.write_text("")
        self._plan(cycles=0)
        self.assertEqual(self._launches()[0][0], cycled)

    def test_a_live_same_name_session_blocks_the_relaunch(self):
        (self.home / "sessions" / "1.json").write_text(
            json.dumps({"pid": os.getpid(), "name": self.name, "sessionId": "x"})
        )
        result = self._plan(cycles=1)
        self.assertEqual(result.returncode, 1)
        self.assertIn("not cycling", result.stderr)
        self.assertEqual(len(self._launches()), 1)


if __name__ == "__main__":
    unittest.main()
