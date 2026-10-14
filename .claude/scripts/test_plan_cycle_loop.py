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
import sys
import tempfile
import unittest
from datetime import datetime
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
INIT = REPO / ".claude" / "shell" / "init.zsh"
sys.path.insert(0, str(REPO / ".claude" / "tools"))

import resolve_session as rs  # noqa: E402

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
_ds_upkeep() { print -r -- upkeep >> "$STATE/upkeep-calls" }
if [[ -n "$MIDNIGHT" ]]; then
  # The clock rolls from the 10th to the 11th once the first client exits.
  date() {
    local d=20260910 dd=10
    [[ -f "$STATE/rolled" ]] && d=20260911 dd=11
    case "$1" in
      +%Y%m%d) print -r -- $d ;;
      +%-d) print -r -- $dd ;;
      *) command date "$@" ;;
    esac
  }
fi
_ds_session() {
  print -r -- "$1|$2|$3|$DS_PLAN_DATE" >> "$CALLS"
  local n=$(wc -l < "$CALLS")
  (( n <= CYCLES )) && print -r -- "$1" > "$STATE/plan-cycle-pending"
  [[ -n "$MIDNIGHT" ]] && : > "$STATE/rolled"
  return ${CLIENT_RC:-0}
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

    def _plan(
        self, cycles: int, client_rc: int = 0, midnight: bool = False
    ) -> subprocess.CompletedProcess:
        env = {**os.environ}
        # A suite run from inside a planning session inherits the launcher's
        # DS_PLAN_DATE, which would mask a launcher that stopped passing it.
        for inherited in ("DS_PLAN_DATE", "MIDNIGHT"):
            env.pop(inherited, None)
        if midnight:
            env["MIDNIGHT"] = "1"
        env |= {
            "CLIENT_RC": str(client_rc),
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

    def test_upkeep_runs_once_before_the_loop_not_per_cycle(self):
        self.assertEqual(self._plan(cycles=2).returncode, 0)
        self.assertEqual(len(self._launches()), 3)
        upkeep = (self.state / "upkeep-calls").read_text().splitlines()
        self.assertEqual(upkeep, ["upkeep"])

    def test_a_cycle_relaunches_under_a_fresh_id_and_the_same_name(self):
        result = self._plan(cycles=1)
        self.assertEqual(result.returncode, 0, result.stderr)
        (sid0, name0, prompt0, date0), (sid1, name1, prompt1, date1) = self._launches()
        # The shell seed and resolve_session's must name the same sessions,
        # or plan_cycle.py cannot find its own transcript or client.
        self.assertEqual(sid0, rs.daily_session_id("plan", self.day, 0))
        self.assertEqual(sid1, rs.daily_session_id("plan", self.day, 1))
        self.assertEqual((name0, name1), (self.name, self.name))
        self.assertEqual((prompt0, prompt1), ("/plan", "/plan"))
        self.assertEqual((date0, date1), (self.day, self.day))
        counter = self.state / f"plan-cycle-{self.day}"
        self.assertEqual(counter.read_text().strip(), "1")
        self.assertFalse((self.state / "plan-cycle-pending").exists())

    def test_a_later_reopen_resumes_the_latest_cycle(self):
        self._plan(cycles=1)
        cycled = self._launches()[-1][0]
        self.calls.write_text("")
        self._plan(cycles=0)
        self.assertEqual(self._launches()[0][0], cycled)

    def test_a_cycle_past_midnight_keeps_the_launch_day(self):
        # Worker addressing must not change: the relaunch stays `plan-10`, on
        # the 10th's ids, even though the clock now reads the 11th.
        result = self._plan(cycles=1, midnight=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        (sid0, name0, _, date0), (sid1, name1, _, date1) = self._launches()
        self.assertEqual((name0, name1), ("plan-10", "plan-10"))
        self.assertEqual((date0, date1), ("20260910", "20260910"))
        self.assertEqual(sid0, rs.daily_session_id("plan", "20260910", 0))
        self.assertEqual(sid1, rs.daily_session_id("plan", "20260910", 1))
        self.assertEqual((self.state / "plan-cycle-20260910").read_text().strip(), "1")

    def test_a_plain_exit_returns_the_clients_status(self):
        self.assertEqual(self._plan(cycles=0, client_rc=3).returncode, 3)
        self.assertEqual(len(self._launches()), 1)

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
