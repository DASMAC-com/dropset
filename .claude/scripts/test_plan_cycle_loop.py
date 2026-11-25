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
  # What a child of the launch sees: exported values only.
  zsh -fc 'print -r -- "${DS_FLEET_TOTAL-}|${CARGO_BUILD_JOBS-}"' >> "$STATE/knobs"
  local n=$(wc -l < "$CALLS")
  (( n <= CYCLES )) && print -r -- "$1" > "$STATE/$VERB-cycle-pending"
  [[ -n "$MIDNIGHT" ]] && : > "$STATE/rolled"
  return ${CLIENT_RC:-0}
}
$VERB
"""


#: The fleet and cargo knobs a launch requires (`_ds_fleet_knobs`).
_KNOBS = {
    "DS_FLEET_TOTAL": "16",
    "DS_FLEET_CPU_LOAD_MAX": "16",
    "DS_CARGO_LANES": "1",
    "CARGO_BUILD_JOBS": "4",
}


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
        knobs: dict[str, str] | None = None,
        profile: str | None = None,
    ) -> subprocess.CompletedProcess:
        # A suite run from inside a planning session inherits the launcher's
        # DS_PLAN_DATE, which would mask a launcher that stopped passing it;
        # `hermetic_env` drops it with every other `DS_*`.
        env = hermetic_env(self.home, "MIDNIGHT", "CARGO_BUILD_JOBS")
        if midnight:
            env["MIDNIGHT"] = "1"
        # The tab's shell: every knob the relaunch gate needs, unless the case
        # says otherwise. `ZDOTDIR` is the claude home, so the profile the
        # relaunch re-reads is `<home>/.zshrc`; absent unless the case writes one.
        env |= _KNOBS if knobs is None else knobs
        if profile is not None:
            (self.home / ".zshrc").write_text(profile)
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

    def test_analyze_is_a_daily_role_of_its_own(self):
        result = self._plan(cycles=0, verb="analyze")
        self.assertEqual(result.returncode, 0, result.stderr)
        seed = f"dropset-analyze-{self.day}".encode()
        raw = hashlib.md5(seed).hexdigest()  # noqa: S324 - an id, not a secret
        sid = f"{raw[:8]}-{raw[8:12]}-{raw[12:16]}-{raw[16:20]}-{raw[20:]}"
        self.assertEqual(
            self._launches(),
            [[sid, f"analyze-{datetime.now().day}", "/analyze", self.day, ""]],
        )
        roles = (self.state / "role-calls").read_text().splitlines()
        self.assertEqual(roles, ["role:analyze", "role:"])
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

    def test_a_knob_raised_since_the_shell_opened_reaches_the_relaunch(self):
        # The operator raised the load limit after the tab opened: the first
        # client ran on the shell's 16, the cycled one must read the 18.
        result = self._plan(
            cycles=1,
            profile="export DS_FLEET_TOTAL=18\nCARGO_BUILD_JOBS=6\n",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self._launches()), 2)
        seen = (self.state / "knobs").read_text().splitlines()
        self.assertEqual(seen, ["16|4", "18|6"])
        self.assertIn("DS_FLEET_TOTAL: this shell holds 16", result.stderr)

    def test_a_knob_still_missing_refuses_the_relaunch_by_name(self):
        # A shell older than the knob, and a profile that does not set it
        # either: the relaunch is refused, never started unable to dispatch.
        knobs = {k: v for k, v in _KNOBS.items() if k != "CARGO_BUILD_JOBS"}
        result = self._plan(cycles=1, knobs=knobs, profile="DS_FLEET_TOTAL=16\n")
        self.assertEqual(result.returncode, 1)
        self.assertIn("CARGO_BUILD_JOBS is unset", result.stderr)
        self.assertIn("not cycling", result.stderr)
        self.assertEqual(len(self._launches()), 1)

    def test_turning_the_pool_off_in_the_profile_lifts_the_cargo_pair(self):
        knobs = {k: v for k, v in _KNOBS.items() if k != "CARGO_BUILD_JOBS"}
        result = self._plan(cycles=1, knobs=knobs, profile="DS_CARGO_POOL=0\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self._launches()), 2)

    def test_an_unreadable_profile_relaunches_on_the_shells_knobs(self):
        result = self._plan(cycles=1)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("unreadable; relaunching on this shell's knobs", result.stderr)
        self.assertEqual(len(self._launches()), 2)

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
        # it into a second client. The scheduler and the analyst both refuse.
        for verb, kind, name in (
            ("plan", "plan", self.name),
            ("analyze", "analyze", f"analyze-{datetime.now().day}"),
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
#: day's daily role — with every outward call recorded. `python3` answers the
#: stamp read from `$RAN` and the dispatch with `$DISPATCH_OUT` / `$DISPATCH_RC`.
_PRELAUNCH_SCRIPT = """
source "$INIT" 2>/dev/null
_DS_REPO="$REPO" _DS_SUBSTRATE_DIR="$STATE"
rec() { print -r -- "$*" >> "$STATE/calls" }
_ds_substrate_unset() { : }
_ds_aws_login() { rec aws; return ${AWS_RC:-0} }
_ds_upkeep() { rec upkeep }
_ds_daily_role() { rec "role $*" }
analyze() { rec "analyze $* [${DS_ANALYZE_SCHEDULER}]" }
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
            "DISPATCH_OUT": "curate (free) -> /dev/a\nanalyze (free) -> /dev/b",
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

    def test_a_fresh_day_runs_the_pass_then_opens_curator_and_analyst(self):
        proc, calls = self._plan()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("plan: pre-launch", proc.stderr)
        self.assertEqual(calls, ["aws", "upkeep", "dispatch curate + analyze"])
        self.assertEqual(self.marker.read_text().strip(), "plan")

    def test_the_pre_launch_names_the_tab_it_waits_on(self):
        proc, _ = self._plan()
        day = datetime.now().strftime("%-d")
        self.assertIn(f"waiting on curate-{day}'s state of play", proc.stderr)

    def test_the_substrate_word_rides_the_marker_to_the_analyst(self):
        # The scheduler opens last, from the analyst's tab, so its verb is
        # recorded for the `analyze` helper rather than dispatched here.
        _, calls = self._plan("anthropic")
        self.assertIn("dispatch curate + analyze", calls)
        self.assertEqual(self.marker.read_text().strip(), "plan anthropic")

    def test_a_refused_login_stops_before_the_pass_and_leaves_the_day_fresh(self):
        proc, calls = self._plan(AWS_RC="1")
        self.assertEqual(proc.returncode, 1)
        self.assertEqual(calls, ["aws"])
        self.assertFalse(self.marker.exists())

    def test_an_analyst_tab_that_did_not_open_runs_here(self):
        proc, calls = self._plan(DISPATCH_OUT="curate (free) -> /dev/a")
        self.assertEqual(calls[-1], "analyze  []")
        self.assertIn("running it in this tab", proc.stderr)

    def test_with_neither_tab_the_analyst_runs_here_and_brings_the_curator(self):
        # A driver failure lists the tabs it DID open on stderr, indented.
        proc, calls = self._plan(
            DISPATCH_OUT="session-dispatch: iTerm went away\n  analyze",
            DISPATCH_RC="1",
        )
        self.assertEqual(calls[-1], "analyze  []")
        self.assertIn("neither tab opened (dispatch exit 1)", proc.stderr)
        self.assertIn("it dispatches the curator first", proc.stderr)

    def test_a_partial_batch_counts_the_tabs_the_driver_did_open(self):
        _, calls = self._plan(
            DISPATCH_OUT="session-dispatch: 2 tab(s) were already opened\n"
            "  curate -> /dev/a\n  analyze -> tty unknown",
            DISPATCH_RC="1",
        )
        self.assertEqual(calls[-1], "dispatch curate + analyze")

    def test_a_held_analyst_is_said_never_run_here(self):
        # A hold is the machine gate's refusal; a fallback must not undo it.
        proc, calls = self._plan(
            DISPATCH_OUT="curate (free) -> /dev/a\nheld  analyze (free): load 9.1"
        )
        self.assertEqual(calls[-1], "dispatch curate + analyze")
        self.assertIn("held the analyst", proc.stderr)
        # No analyst runs, so nothing will open the scheduler on the curator.
        self.assertNotIn("waiting on curate-", proc.stderr)

    def test_a_held_analyst_without_its_curator_names_curate_to_type(self):
        proc, calls = self._plan(DISPATCH_OUT="held  analyze (free): load 9.1")
        self.assertEqual(calls[-1], "dispatch curate + analyze")
        self.assertIn("type `curate` in a new tab", proc.stderr)
        self.assertNotIn("the analyst dispatches it first", proc.stderr)

    def test_with_both_held_nothing_runs_here(self):
        proc, calls = self._plan(
            DISPATCH_OUT="held  curate (free): cap 20\nheld  analyze (free): same"
        )
        self.assertEqual(calls[-1], "dispatch curate + analyze")
        self.assertIn("held the curator", proc.stderr)
        self.assertNotIn("neither tab opened", proc.stderr)
        self.assertNotIn("waiting on curate-", proc.stderr)

    def test_an_analyst_without_its_curator_dispatches_it_first(self):
        proc, calls = self._plan(DISPATCH_OUT="analyze (free) -> /dev/b")
        self.assertEqual(calls[-1], "dispatch curate + analyze")
        self.assertIn("the analyst dispatches it first", proc.stderr)

    def test_a_stamped_day_is_warm_and_runs_the_scheduler_here(self):
        proc, calls = self._plan(RAN='{"ran_today": true}')
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(calls, ["role plan schedule "])
        self.assertNotIn("pre-launch", proc.stderr)

    def test_after_the_pre_launch_the_scheduler_tabs_plan_is_warm(self):
        # The pass failed to stamp, but the day's marker stops a second
        # pre-launch — the scheduler tab's own `plan` would otherwise loop.
        self.marker.write_text("")
        _, calls = self._plan()
        self.assertEqual(calls, ["role plan schedule "])

    def test_a_scheduler_session_of_todays_makes_the_day_warm(self):
        sid = rs.daily_session_id("plan", self.day, 0)
        (self.home / "projects" / "x").mkdir(parents=True)
        (self.home / "projects" / "x" / f"{sid}.jsonl").write_text("")
        _, calls = self._plan()
        self.assertEqual(calls, ["role plan schedule "])


#: The real `analyze` up to its client, which records the scheduler verb the
#: session would dispatch on the curator's `state of play written`.
_ANALYZE_SCRIPT = """
source "$INIT" 2>/dev/null
_DS_REPO="$REPO" _DS_SUBSTRATE_DIR="$STATE"
_ds_tier() { print -r -- model; print -r -- anthropic }
_ds_substrate_enter() { : }
_ds_iterm_role() { : }
_ds_session() { print -r -- "$2|$DS_ANALYZE_SCHEDULER" >> "$STATE/calls" }
analyze
"""


@unittest.skipUnless(ZSH, _NEEDS_ZSH)
class AnalyzeHelper(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.state = Path(tmp.name) / "state"
        self.state.mkdir()
        self.home = Path(tmp.name) / "claude"
        self.home.mkdir()
        day = datetime.now().strftime("%Y%m%d")
        self.marker = self.state / f"plan-prelaunch-{day}"

    def _scheduler(self, **extra: str) -> str:
        env = hermetic_env(self.home)
        env |= {"INIT": str(INIT), "REPO": str(REPO), "STATE": str(self.state)}
        env |= extra
        proc = subprocess.run(
            [ZSH, "-c", _ANALYZE_SCRIPT],
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        (call,) = (self.state / "calls").read_text().splitlines()
        return call.split("|", 1)[1]

    def test_the_pre_launch_marker_names_the_scheduler_verb(self):
        self.marker.write_text("plan anthropic\n")
        self.assertEqual(self._scheduler(), "plan anthropic")

    def test_a_callers_verb_wins_over_the_marker(self):
        self.marker.write_text("plan anthropic\n")
        self.assertEqual(
            self._scheduler(DS_ANALYZE_SCHEDULER="plan bedrock"), "plan bedrock"
        )

    def test_no_marker_or_an_empty_one_defaults_to_plan(self):
        self.assertEqual(self._scheduler(), "plan")
        (self.state / "calls").unlink()
        # A pre-launch from before the marker carried the verb wrote it empty.
        self.marker.write_text("")
        self.assertEqual(self._scheduler(), "plan")


#: The real `_ds_upkeep`, with `python3` and the secrets fetch stubbed: the
#: helper must return 0 whatever the pass does, and skip it on the off switch.
#: The manifest names two variables: one the profile assigns but never exports,
#: one it exports.
_UPKEEP_SCRIPT = """
source "$INIT" 2>/dev/null
_DS_REPO="$REPO"
_ds_secrets() { : }
python3() {
  print -r -- "$*" >> "$CALLS"
  [[ "$*" == *--names* ]] && { print -l -- DS_PROBE_KNOB DS_PROBE_EXPORTED; return 0 }
  print -r -- "SEEN=$(printenv DS_PROBE_KNOB)" >> "$CALLS"
  print -r -- "UNEXPORTED=$(printenv DS_CONFIG_UNEXPORTED)" >> "$CALLS"
  # A Ctrl-C reaches the whole foreground group; signal the shell as a tty would.
  [[ -n "$INTERRUPT" ]] && { kill -INT $$; return 130 }
  return ${PASS_RC:-0}
}
DS_PROBE_KNOB=plain
export DS_PROBE_EXPORTED=1
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

    def test_the_pass_is_told_which_variables_only_it_reaches(self):
        # The plain assignment is named, so the check can flag a tool-read
        # row; the exported one is not.
        _, calls = self._upkeep()
        self.assertIn("UNEXPORTED=DS_PROBE_KNOB", calls.splitlines())

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
