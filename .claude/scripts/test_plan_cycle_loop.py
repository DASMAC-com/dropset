"""The `plan` launcher's cycle loop, run for real with the client stubbed.

A cycled planning session leaves `plan-cycle-pending` and ends its client; the
loop must relaunch in place under a fresh id and the same name, bump the day's
counter, and refuse when a live session already holds the name. Each case runs
the real `plan` with `_ds_session` replaced by a recorder.

Run with: ``python3 -m unittest discover -s .claude/scripts``
"""

from __future__ import annotations

import hashlib
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
from zsh_test_env import hermetic_env  # noqa: E402

# No case may reach a real iTerm tab, whatever it forgets to stub: inherited by
# every zsh this spawns (`iterm_api.DISABLED_ENV`).
os.environ["DS_ITERM_DISABLED"] = "1"

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
# Every case here is a warm day; the pre-launch has its own class below.
_ds_plan_fresh_day() { return 1 }
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
_ds_iterm_role() { print -r -- "role:$1" >> "$STATE/role-calls" }
_ds_session() {
  print -r -- "$1|$2|$3|$DS_PLAN_DATE|$DS_DAILY_KIND" >> "$CALLS"
  local n=$(wc -l < "$CALLS")
  (( n <= CYCLES )) && print -r -- "$1" > "$STATE/$VERB-cycle-pending"
  [[ -n "$MIDNIGHT" ]] && : > "$STATE/rolled"
  return ${CLIENT_RC:-0}
}
$VERB
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
        self.name = f"schedule-{datetime.now().day}"

    def _plan(
        self,
        cycles: int,
        client_rc: int = 0,
        midnight: bool = False,
        verb: str = "plan",
    ) -> subprocess.CompletedProcess:
        # A suite run from inside a planning session inherits the launcher's
        # DS_PLAN_DATE, which would mask a launcher that stopped passing it;
        # `hermetic_env` drops it with every other `DS_*`.
        env = hermetic_env(self.home, "MIDNIGHT")
        if midnight:
            env["MIDNIGHT"] = "1"
        env |= {
            "VERB": verb,
            "CLIENT_RC": str(client_rc),
            "INIT": str(INIT),
            "REPO": str(REPO),
            "STATE": str(self.state),
            "CALLS": str(self.calls),
            "CYCLES": str(cycles),
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

    def test_a_warm_day_scheduler_runs_no_pass(self):
        # The pass is the pre-launch's alone, so no cycle repeats it either.
        self.assertEqual(self._plan(cycles=2).returncode, 0)
        self.assertEqual(len(self._launches()), 3)
        self.assertFalse((self.state / "upkeep-calls").exists())

    def test_launch_is_a_daily_seat_of_its_own(self):
        result = self._plan(cycles=0, verb="launch")
        self.assertEqual(result.returncode, 0, result.stderr)
        seed = f"dropset-launch-{self.day}".encode()
        raw = hashlib.md5(seed).hexdigest()  # noqa: S324 - an id, not a secret
        sid = f"{raw[:8]}-{raw[8:12]}-{raw[12:16]}-{raw[16:20]}-{raw[20:]}"
        self.assertEqual(
            self._launches(),
            [[sid, f"launch-{datetime.now().day}", "/launch", self.day, ""]],
        )
        roles = (self.state / "role-calls").read_text().splitlines()
        self.assertEqual(roles, ["role:launch", "role:"])
        self.assertFalse((self.state / "upkeep-calls").exists())

    def test_a_cycle_relaunches_under_a_fresh_id_and_the_same_name(self):
        result = self._plan(cycles=1)
        self.assertEqual(result.returncode, 0, result.stderr)
        (sid0, name0, prompt0, date0, _), (sid1, name1, prompt1, date1, _) = (
            self._launches()
        )
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
        # Worker addressing must not change: the relaunch stays `schedule-10`,
        # on the 10th's ids, even though the clock now reads the 11th.
        result = self._plan(cycles=1, midnight=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        (sid0, name0, _, date0, _), (sid1, name1, _, date1, _) = self._launches()
        self.assertEqual((name0, name1), ("schedule-10", "schedule-10"))
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

    def test_a_reopen_refuses_while_the_session_has_a_live_client(self):
        # Measured: a second `plan` resumed the live scheduler by id and forked
        # it into a second client. The seat and the launcher both refuse.
        for verb, kind, name in (
            ("plan", "plan", self.name),
            ("launch", "launch", f"launch-{datetime.now().day}"),
        ):
            with self.subTest(verb=verb):
                seed = f"dropset-{kind}-{self.day}".encode()
                raw = hashlib.md5(seed).hexdigest()  # noqa: S324 - an id
                sid = f"{raw[:8]}-{raw[8:12]}-{raw[12:16]}-{raw[16:20]}-{raw[20:]}"
                project = self.home / "projects" / "x"
                project.mkdir(parents=True, exist_ok=True)
                (project / f"{sid}.jsonl").write_text("")
                (self.home / "sessions" / "1.json").write_text(
                    json.dumps({"pid": os.getpid(), "name": name, "sessionId": sid})
                )
                self.calls.write_text("")
                result = self._plan(cycles=0, verb=verb)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn("already has a live client", result.stderr)
                self.assertEqual(self._launches(), [])

    def test_curate_cycles_on_its_own_counter_without_upkeep(self):
        result = self._plan(cycles=1, verb="curate")
        self.assertEqual(result.returncode, 0, result.stderr)
        launches = self._launches()
        name = f"curate-{datetime.now().day}"
        self.assertEqual(
            launches,
            [
                [
                    rs.daily_session_id("curate", self.day, cycle),
                    name,
                    "/curate",
                    self.day,
                    "curate",
                ]
                for cycle in (0, 1)
            ],
        )
        counter = self.state / f"curate-cycle-{self.day}"
        self.assertEqual(counter.read_text().strip(), "1")
        self.assertFalse((self.state / f"plan-cycle-{self.day}").exists())
        self.assertFalse((self.state / "curate-cycle-pending").exists())
        # Only the pre-launch runs upkeep; the role is set and cleared.
        self.assertFalse((self.state / "upkeep-calls").exists())
        roles = (self.state / "role-calls").read_text().splitlines()
        self.assertEqual(roles, ["role:curate", "role:"])

    def test_plan_marks_its_tab_and_its_kind(self):
        self.assertEqual(self._plan(cycles=0).returncode, 0)
        self.assertEqual(self._launches()[0][4], "plan")
        roles = (self.state / "role-calls").read_text().splitlines()
        self.assertEqual(roles, ["role:plan", "role:"])


#: The real `plan` up to its two exits — the pre-launch's dispatch and the warm
#: day's seat — with every outward call recorded. `python3` answers the
#: stamp read from `$RAN` and the dispatch with `$DISPATCH_OUT` / `$DISPATCH_RC`.
_PRELAUNCH_SCRIPT = """
source "$INIT" 2>/dev/null
_DS_REPO="$REPO" _DS_SUBSTRATE_DIR="$STATE"
rec() { print -r -- "$*" >> "$STATE/calls" }
_ds_substrate_unset() { : }
_ds_aws_login() { rec aws; return ${AWS_RC:-0} }
_ds_upkeep() { rec upkeep }
_ds_daily_seat() { rec "seat $*" }
launch() { rec "launch $* [${DS_LAUNCH_SCHEDULER}]" }
python3() {
  if [[ "$*" == *session_dispatch.py* ]]; then
    rec "dispatch ${@[2,-1]}"
    print -r -- "$DISPATCH_OUT"
    return ${DISPATCH_RC:-0}
  fi
  [[ "$*" == *ran-today* ]] && print -r -- "$RAN"
}
plan $WORD
"""


@unittest.skipUnless(ZSH, _NEEDS_ZSH)
class PreLaunch(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.state = self.root / "state"
        self.state.mkdir()
        self.home = self.root / "claude"
        self.home.mkdir()
        self.day = datetime.now().strftime("%Y%m%d")
        self.marker = self.state / f"plan-prelaunch-{self.day}"

    def _plan(self, word: str = "", **extra: str):
        env = hermetic_env(self.home)
        env |= {
            "INIT": str(INIT),
            "REPO": str(REPO),
            "STATE": str(self.state),
            "WORD": word,
            "DISPATCH_OUT": "curate (free) -> /dev/a\nlaunch (free) -> /dev/b",
            **extra,
        }
        proc = subprocess.run(
            [ZSH, "-c", _PRELAUNCH_SCRIPT],
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )
        path = self.state / "calls"
        calls = path.read_text().splitlines() if path.exists() else []
        return proc, calls

    def test_a_fresh_day_runs_the_pass_then_opens_curator_and_launcher(self):
        proc, calls = self._plan()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("plan: pre-launch", proc.stderr)
        self.assertEqual(calls, ["aws", "upkeep", "dispatch curate + launch"])
        self.assertEqual(self.marker.read_text().strip(), "plan")

    def test_the_pre_launch_names_the_tab_it_waits_on(self):
        proc, _ = self._plan()
        day = datetime.now().strftime("%-d")
        self.assertIn(f"waiting on curate-{day}'s state of play", proc.stderr)

    def test_the_substrate_word_rides_the_marker_to_the_launcher(self):
        # The scheduler opens last, from the launcher's tab, so its verb is
        # recorded for the `launch` helper rather than dispatched here.
        _, calls = self._plan("anthropic")
        self.assertIn("dispatch curate + launch", calls)
        self.assertEqual(self.marker.read_text().strip(), "plan anthropic")

    def test_a_refused_login_stops_before_the_pass_and_leaves_the_day_fresh(self):
        proc, calls = self._plan(AWS_RC="1")
        self.assertEqual(proc.returncode, 1)
        self.assertEqual(calls, ["aws"])
        self.assertFalse(self.marker.exists())

    def test_a_launcher_tab_that_did_not_open_runs_here(self):
        proc, calls = self._plan(DISPATCH_OUT="curate (free) -> /dev/a")
        self.assertEqual(calls[-1], "launch  []")
        self.assertIn("running it in this tab", proc.stderr)

    def test_with_neither_tab_the_launcher_runs_here_and_brings_the_curator(self):
        # A driver failure lists the tabs it DID open on stderr, indented.
        proc, calls = self._plan(
            DISPATCH_OUT="session-dispatch: iTerm went away\n  launch",
            DISPATCH_RC="1",
        )
        self.assertEqual(calls[-1], "launch  []")
        self.assertIn("neither tab opened (dispatch exit 1)", proc.stderr)
        self.assertIn("it dispatches the curator first", proc.stderr)

    def test_a_partial_batch_counts_the_tabs_the_driver_did_open(self):
        _, calls = self._plan(
            DISPATCH_OUT="session-dispatch: 2 tab(s) were already opened\n"
            "  curate -> /dev/a\n  launch -> tty unknown",
            DISPATCH_RC="1",
        )
        self.assertEqual(calls[-1], "dispatch curate + launch")

    def test_a_held_launcher_is_said_never_run_here(self):
        # A hold is the machine gate's refusal; a fallback must not undo it.
        proc, calls = self._plan(
            DISPATCH_OUT="curate (free) -> /dev/a\nheld  launch (free): load 9.1"
        )
        self.assertEqual(calls[-1], "dispatch curate + launch")
        self.assertIn("held the launcher", proc.stderr)

    def test_with_both_held_nothing_runs_here(self):
        proc, calls = self._plan(
            DISPATCH_OUT="held  curate (free): cap 20\nheld  launch (free): same"
        )
        self.assertEqual(calls[-1], "dispatch curate + launch")
        self.assertIn("held the curator", proc.stderr)
        self.assertNotIn("neither tab opened", proc.stderr)

    def test_a_launcher_without_its_curator_dispatches_it_first(self):
        proc, calls = self._plan(DISPATCH_OUT="launch (free) -> /dev/b")
        self.assertEqual(calls[-1], "dispatch curate + launch")
        self.assertIn("the launcher dispatches it first", proc.stderr)

    def test_a_stamped_day_is_warm_and_runs_the_scheduler_here(self):
        proc, calls = self._plan(RAN='{"ran_today": true}')
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(calls, ["seat plan schedule "])
        self.assertNotIn("pre-launch", proc.stderr)

    def test_after_the_pre_launch_the_scheduler_tabs_plan_is_warm(self):
        # The pass failed to stamp, but the day's marker stops a second
        # pre-launch — the scheduler tab's own `plan` would otherwise loop.
        self.marker.write_text("")
        _, calls = self._plan()
        self.assertEqual(calls, ["seat plan schedule "])

    def test_a_scheduler_session_of_todays_makes_the_day_warm(self):
        sid = rs.daily_session_id("plan", self.day, 0)
        (self.home / "projects" / "x").mkdir(parents=True)
        (self.home / "projects" / "x" / f"{sid}.jsonl").write_text("")
        _, calls = self._plan()
        self.assertEqual(calls, ["seat plan schedule "])


#: The real `_ds_upkeep`, with `python3` and the secrets fetch stubbed: the
#: helper must return 0 whatever the pass does, and skip it on the off switch.
#: The manifest names one variable, which the profile assigns but never exports.
_UPKEEP_SCRIPT = """
source "$INIT" 2>/dev/null
_DS_REPO="$REPO"
_ds_secrets() { : }
python3() {
  print -r -- "$*" >> "$CALLS"
  [[ "$*" == *--names* ]] && { print -r -- DS_PROBE_KNOB; return 0 }
  print -r -- "SEEN=$(printenv DS_PROBE_KNOB)" >> "$CALLS"
  # A Ctrl-C reaches the whole foreground group; signal the shell as a tty would.
  [[ -n "$INTERRUPT" ]] && { kill -INT $$; return 130 }
  return ${PASS_RC:-0}
}
DS_PROBE_KNOB=plain
_ds_upkeep
print -r -- "AFTER=$(printenv DS_PROBE_KNOB)" >> "$CALLS"
"""


@unittest.skipUnless(ZSH, _NEEDS_ZSH)
class UpkeepHelper(unittest.TestCase):
    def _upkeep(self, **extra: str) -> tuple[subprocess.CompletedProcess, str]:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        calls = Path(tmp.name) / "calls"
        calls.touch()
        env = hermetic_env(Path(tmp.name) / "claude-home")
        env |= {"INIT": str(INIT), "REPO": str(REPO), "CALLS": str(calls), **extra}
        proc = subprocess.run(
            [ZSH, "-c", _UPKEEP_SCRIPT],
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )
        return proc, calls.read_text()

    def test_runs_the_pass_with_the_stamp_gate(self):
        proc, calls = self._upkeep()
        self.assertEqual(proc.returncode, 0)
        self.assertIn("upkeep.py run --if-stale", calls)

    def test_a_plain_profile_assignment_reaches_the_pass_and_only_the_pass(self):
        # The config step reads its environment; a profile's unexported model
        # id must reach it, and the export must not outlive the helper.
        _, calls = self._upkeep()
        self.assertIn("SEEN=plain", calls.splitlines())
        self.assertIn("AFTER=", calls.splitlines())

    def test_a_failed_pass_warns_and_still_returns_zero(self):
        proc, _ = self._upkeep(PASS_RC="3")
        self.assertEqual(proc.returncode, 0)
        self.assertIn("failed (exit 3)", proc.stderr)

    def test_ctrl_c_skips_the_pass_and_still_returns_zero(self):
        proc, _ = self._upkeep(INTERRUPT="1")
        self.assertEqual(proc.returncode, 0)
        self.assertIn("upkeep interrupted", proc.stderr)
        self.assertNotIn("failed", proc.stderr)

    def test_the_off_switch_runs_nothing(self):
        proc, calls = self._upkeep(DS_SKIP_UPKEEP="1")
        self.assertEqual(proc.returncode, 0)
        # Only the script's own trailing probe line; no python3 call at all.
        self.assertEqual(calls, "AFTER=\n")
        self.assertIn("DS_SKIP_UPKEEP=1", proc.stderr)


if __name__ == "__main__":
    unittest.main()
