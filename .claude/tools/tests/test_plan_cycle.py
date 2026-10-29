"""Tests for `.claude/tools/plan_cycle.py` — the planning hub's cycle mechanics.

Run with: ``python3 -m unittest discover -s .claude/tools/tests``
"""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import plan_cycle as pc  # noqa: E402
import resolve_session as rs  # noqa: E402
import session_metrics as sm  # noqa: E402

DATE = "20260910"

#: Placeholder model ids with rows patched in, so no test pins a real model.
#: The advisor row carries the cache-read rate the dollar threshold derives
#: from; round rates elsewhere keep the expected sums exact.
ADVISOR = "test-advisor"
WORKER = "test-worker"
RATES = {
    ADVISOR: sm.Rates(10.0, 50.0, pc.ADVISOR_CACHE_READ, 20.0, verified=None),
    WORKER: sm.Rates(4.0, 20.0, 0.5, 8.0, verified=None),
}


def assistant(inp: int, read: int, write: int, model: str | None = None) -> dict:
    message = {
        "usage": {
            "input_tokens": inp,
            "cache_read_input_tokens": read,
            "cache_creation_input_tokens": write,
            "output_tokens": 999,
        }
    }
    if model is not None:
        message["model"] = model
    return {"type": "assistant", "message": message}


def priced():
    return mock.patch.dict(sm.RATES_BY_MODEL, RATES)


class Fixture:
    def __init__(self, tmp: str):
        self.root = Path(tmp)
        self.repo = self.root / "repo"
        self.home = self.root / "claude"
        self.repo.mkdir()
        self.env = mock.patch.dict(os.environ, {"CLAUDE_CONFIG_DIR": str(self.home)})

    def __enter__(self):
        self.env.start()
        # A suite run from inside a scheduler session inherits the launcher's
        # date; patch.dict restores it on exit.
        os.environ.pop("DS_PLAN_DATE", None)
        os.environ.pop(pc.KIND_ENV, None)
        return self

    def __exit__(self, *exc):
        self.env.stop()

    def transcript(self, records: list[dict], cycle: int = 0) -> Path:
        sid = rs.daily_session_id("plan", DATE, cycle)
        path = self.home / "projects" / rs.slugify(self.repo) / f"{sid}.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("".join(json.dumps(r) + "\n" for r in records))
        return path

    def set_cycle(self, n: int) -> None:
        counter = rs.cycle_counter_path(self.repo, DATE)
        counter.parent.mkdir(parents=True, exist_ok=True)
        counter.write_text(f"{n}\n")

    def register(self, pid: int, name: str, sid: str, started: int = 0) -> None:
        sessions = self.home / "sessions"
        sessions.mkdir(parents=True, exist_ok=True)
        (sessions / f"{pid}.json").write_text(
            json.dumps(
                {"pid": pid, "name": name, "sessionId": sid, "startedAt": started}
            )
        )

    def cycle_cli(self) -> tuple[int, list]:
        """Run `cycle` with every pid live and every signal recorded."""
        sent = []
        with mock.patch.object(pc.os, "kill", lambda pid, sig: sent.append((pid, sig))):
            with redirect_stdout(io.StringIO()):
                rc = pc.run(
                    ["plan_cycle.py", "--repo", str(self.repo), "--date", DATE, "cycle"]
                )
        return rc, [pid for pid, sig in sent if sig == pc.signal.SIGTERM]


class PricedCheckTests(unittest.TestCase):
    def test_both_dollar_figures_on_a_priced_session(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx, priced():
            fx.transcript(
                [
                    assistant(0, 0, 1_000_000, WORKER),
                    assistant(0, 1_000_000, 100_000, WORKER),
                ]
            )
            got = pc.check(fx.repo, DATE)
        self.assertEqual(got["model"], WORKER)
        self.assertEqual(got["prefix_tokens"], 1_100_000)
        # 1.1M read at 0.5, plus the last 100k write at 8.0 for the next turn.
        self.assertAlmostEqual(got["replay_usd"], 0.55)
        self.assertAlmostEqual(got["next_turn_usd"], 0.55 + 0.8)
        # Writes 1.1M at 8.0, reads 1M at 0.5, output 1998 at 20.0.
        self.assertAlmostEqual(got["session_usd"], 8.8 + 0.5 + 0.03996)
        self.assertNotIn("fallback", got)

    def test_due_fires_at_the_derived_threshold_on_the_advisor_rate(self):
        # A prefix-only turn on the advisor rate trips at the ratified 250k.
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx, priced():
            fx.transcript([assistant(0, pc.TOKEN_THRESHOLD - 1, 0, ADVISOR)])
            self.assertFalse(pc.check(fx.repo, DATE)["due"])
            fx.transcript([assistant(0, pc.TOKEN_THRESHOLD, 0, ADVISOR)])
            got = pc.check(fx.repo, DATE)
        self.assertTrue(got["due"])
        self.assertEqual(got["usd_threshold"], 0.06875)

    def test_due_fires_on_dollars_not_tokens(self):
        # A pricier read rate trips well under 250k tokens.
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx, priced():
            fx.transcript([assistant(0, 150_000, 0, WORKER)])
            got = pc.check(fx.repo, DATE)
        self.assertLess(got["prefix_tokens"], pc.TOKEN_THRESHOLD)
        self.assertTrue(got["due"])

    def test_the_write_term_does_not_count_toward_due(self):
        # A fresh session pays the write too, so a big one tips the marginal
        # figure past the threshold without making the cycle due.
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx, priced():
            fx.transcript([assistant(0, 200_000, 10_000, ADVISOR)])
            got = pc.check(fx.repo, DATE)
        self.assertGreater(got["next_turn_usd"], got["usd_threshold"])
        self.assertFalse(got["due"])

    def test_a_failed_last_request_does_not_hide_the_prefix(self):
        # A failed request is recorded as an all-zero `<synthetic>` record.
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx, priced():
            fx.transcript(
                [
                    assistant(0, pc.TOKEN_THRESHOLD, 0, ADVISOR),
                    {
                        "type": "assistant",
                        "message": {
                            "model": "<synthetic>",
                            "usage": {"input_tokens": 0, "output_tokens": 0},
                        },
                    },
                ]
            )
            got = pc.check(fx.repo, DATE)
        self.assertEqual(got["model"], ADVISOR)
        self.assertEqual(got["prefix_tokens"], pc.TOKEN_THRESHOLD)
        self.assertTrue(got["due"])

    def test_an_unpriced_sub_agent_withholds_only_the_session_figure(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx, priced():
            path = fx.transcript([assistant(0, 10, 0, WORKER)])
            sid = path.stem
            sub = path.parent / sid / "subagents" / "agent-x.jsonl"
            sub.parent.mkdir(parents=True)
            sub.write_text(json.dumps(assistant(5, 0, 0, "no-such-model")) + "\n")
            got = pc.check(fx.repo, DATE)
        self.assertIsNone(got["session_usd"])
        self.assertIsNotNone(got["next_turn_usd"])


class FallbackCheckTests(unittest.TestCase):
    """A model with no rate row: due on the token threshold, said so."""

    def test_the_fallback_reports_tokens_and_says_so(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(0, 10, 0, "no-such-model")])
            got = pc.check(fx.repo, DATE)
        self.assertEqual(got["model"], "no-such-model")
        self.assertIsNone(got["replay_usd"])
        self.assertIsNone(got["next_turn_usd"])
        self.assertIsNone(got["session_usd"])
        self.assertIn("no-such-model", got["fallback"])
        self.assertFalse(got["due"])

    def test_prefix_is_the_last_assistant_input_side(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(1, 10, 100), assistant(2, 200, 3000)])
            got = pc.check(fx.repo, DATE)
        self.assertEqual(got["prefix_tokens"], 3202)
        self.assertEqual(got["model"], sm.UNRECORDED_MODEL)
        self.assertFalse(got["due"])

    def test_no_assistant_record_reads_as_an_undue_fallback(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([{"type": "user", "message": {"content": "hi"}}])
            got = pc.check(fx.repo, DATE)
        self.assertEqual(got["prefix_tokens"], 0)
        self.assertIn("fallback", got)
        self.assertFalse(got["due"])

    def test_due_on_tokens(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(0, pc.TOKEN_THRESHOLD, 0)])
            self.assertTrue(pc.check(fx.repo, DATE)["due"])


class CheckTests(unittest.TestCase):
    def test_a_usage_only_in_the_head_is_found_by_the_fallback(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            filler = {"type": "user", "message": {"content": "x" * 4096}}
            fx.transcript([assistant(0, 7, 0)] + [filler] * 400)
            with mock.patch.object(pc, "TAIL_BYTES", 10_000):
                self.assertEqual(pc.check(fx.repo, DATE)["prefix_tokens"], 7)

    def test_the_tail_wins_over_an_older_usage_in_the_head(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            filler = {"type": "user", "message": {"content": "x" * 4096}}
            fx.transcript([assistant(0, 7, 0)] + [filler] * 400 + [assistant(0, 9, 0)])
            with mock.patch.object(pc, "TAIL_BYTES", 10_000):
                self.assertEqual(pc.check(fx.repo, DATE)["prefix_tokens"], 9)

    def test_the_cli_prints_one_json_line(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(0, 3, 0)])
            out = io.StringIO()
            with redirect_stdout(out):
                rc = pc.run(
                    ["plan_cycle.py", "--repo", str(fx.repo), "--date", DATE, "check"]
                )
        self.assertEqual(rc, 0)
        lines = out.getvalue().splitlines()
        self.assertEqual(len(lines), 1)
        self.assertEqual(json.loads(lines[0])["prefix_tokens"], 3)

    def test_the_launch_date_comes_from_the_launcher(self):
        # Past midnight the session must still name the id it was started
        # under, which only the launcher's DS_PLAN_DATE knows.
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(0, 4, 0)])
            out = io.StringIO()
            with mock.patch.dict(os.environ, {"DS_PLAN_DATE": DATE}):
                with redirect_stdout(out):
                    pc.run(["plan_cycle.py", "--repo", str(fx.repo), "check"])
        self.assertEqual(json.loads(out.getvalue())["prefix_tokens"], 4)

    def test_main_maps_a_tool_error_to_exit_2(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            argv = ["plan_cycle.py", "--repo", str(fx.repo), "--date", DATE, "check"]
            with mock.patch.object(pc.sys, "argv", argv):
                with redirect_stderr(io.StringIO()):
                    self.assertEqual(pc.main(), 2)

    def test_the_cycle_counter_selects_the_transcript(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(0, 5, 0)], cycle=0)
            fx.transcript([assistant(0, 9, 0)], cycle=1)
            fx.set_cycle(1)
            self.assertEqual(pc.check(fx.repo, DATE)["prefix_tokens"], 9)

    def test_a_missing_transcript_is_an_error_not_zero(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            with self.assertRaises(pc.PlanCycleError):
                pc.check(fx.repo, DATE)


class SnoozeTests(unittest.TestCase):
    def test_snooze_adds_the_dollar_step_past_the_current_figure(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx, priced():
            fx.transcript([assistant(0, 260_000, 0, ADVISOR)])
            before = pc.check(fx.repo, DATE)
            pc.snooze(fx.repo, DATE)
            got = pc.check(fx.repo, DATE)
        self.assertTrue(before["due"])
        self.assertAlmostEqual(
            got["usd_threshold"], before["replay_usd"] + pc.SNOOZE_USD
        )
        self.assertEqual(got["token_threshold"], 260_000 + pc.SNOOZE_TOKENS)
        self.assertFalse(got["due"])

    def test_the_dollar_step_is_the_token_step_at_the_advisor_rate(self):
        self.assertAlmostEqual(pc.SNOOZE_USD, 0.01375)

    def test_snooze_moves_the_token_threshold_on_the_fallback(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(0, 260_000, 0)])
            pc.snooze(fx.repo, DATE)
            got = pc.check(fx.repo, DATE)
        self.assertEqual(got["token_threshold"], 260_000 + pc.SNOOZE_TOKENS)
        self.assertAlmostEqual(got["usd_threshold"], pc.USD_THRESHOLD + pc.SNOOZE_USD)
        self.assertFalse(got["due"])

    def test_a_retired_message_shape_snooze_reads_as_none(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(0, 1, 0)])
            sid = rs.daily_session_id("plan", DATE, 0)
            state = fx.repo / rs.STATE_DIR
            state.mkdir(parents=True)
            (state / pc.SNOOZE).write_text(
                json.dumps({"sid": sid, "tokens": 9, "messages": 55})
            )
            got = pc.check(fx.repo, DATE)
        self.assertEqual(got["token_threshold"], pc.TOKEN_THRESHOLD)

    def test_a_non_object_snooze_file_reads_as_none(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(0, 1, 0)])
            state = fx.repo / rs.STATE_DIR
            state.mkdir(parents=True)
            (state / pc.SNOOZE).write_text("[]\n")
            got = pc.check(fx.repo, DATE)
        self.assertEqual(got["token_threshold"], pc.TOKEN_THRESHOLD)

    def test_a_snooze_does_not_outlive_its_session(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(0, 1, 0)], cycle=0)
            pc.snooze(fx.repo, DATE)
            fx.transcript([assistant(0, 1, 0)], cycle=1)
            fx.set_cycle(1)
            self.assertEqual(
                pc.check(fx.repo, DATE)["token_threshold"], pc.TOKEN_THRESHOLD
            )


class LiveAndCycleTests(unittest.TestCase):
    def test_a_dead_pid_is_not_live(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.register(os.getpid(), "plan-10", "a")
            fx.register(2**22 + 12345, "plan-10", "b")
            with mock.patch.object(
                pc.os, "kill", side_effect=lambda pid, sig: _probe(pid)
            ):
                live = pc.live_sessions()
        self.assertEqual([s["sessionId"] for s in live], ["a"])

    def test_live_name_exit_codes(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.register(os.getpid(), "plan-10", "a")
            self.assertEqual(pc.run(["plan_cycle.py", "live-name", "plan-10"]), 0)
            self.assertEqual(pc.run(["plan_cycle.py", "live-name", "plan-11"]), 1)

    def test_a_pid_owned_by_another_user_is_not_live(self):
        def kill(pid, sig):
            raise PermissionError(pid)

        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.register(1, "plan-10", "a")
            with mock.patch.object(pc.os, "kill", kill):
                self.assertEqual(pc.live_sessions(), [])

    def test_cycle_writes_the_marker_and_signals_only_its_own_client(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            sid = rs.daily_session_id("plan", DATE, 0)
            # The bystander's file sorts FIRST, so an unfiltered kill hits it.
            fx.register(1111, "eng-1", "other")
            fx.register(4242, "plan-10", sid)
            rc, killed = fx.cycle_cli()
            marker = (fx.repo / rs.STATE_DIR / pc.MARKER).read_text().strip()
        self.assertEqual(rc, 0)
        self.assertEqual(marker, sid)
        self.assertEqual(killed, [4242])

    def test_a_stale_entry_for_the_same_session_is_not_signalled(self):
        # A crashed earlier client of this deterministic id left its file,
        # and its pid is now someone else's: the newest entry is ours.
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            sid = rs.daily_session_id("plan", DATE, 0)
            # The newer client sorts FIRST, so neither first- nor last-file
            # order can pass for the start-time rule.
            fx.register(1111, "plan-10", sid, started=200)
            fx.register(4242, "plan-10", sid, started=100)
            fx.register(5353, "plan-10", sid, started=150)
            _, killed = fx.cycle_cli()
        self.assertEqual(killed, [1111])

    def test_a_malformed_start_time_does_not_raise(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            sid = rs.daily_session_id("plan", DATE, 0)
            fx.register(4242, "plan-10", sid, started="soon")
            _, killed = fx.cycle_cli()
        self.assertEqual(killed, [4242])

    def test_cycle_with_no_client_still_leaves_the_marker(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            with self.assertRaises(pc.PlanCycleError):
                pc.cycle(fx.repo, DATE)
            self.assertTrue((fx.repo / rs.STATE_DIR / pc.MARKER).is_file())


class CuratorKindTests(unittest.TestCase):
    """`curate` cycles on its own counter, marker and snooze file, selected by
    `--kind` or the launcher's `DS_DAILY_KIND`."""

    def test_the_env_kind_cycles_the_curator_alone(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            counter = rs.cycle_counter_path(fx.repo, DATE, "curate")
            counter.parent.mkdir(parents=True, exist_ok=True)
            counter.write_text("2\n")
            sid = rs.daily_session_id("curate", DATE, 2)
            fx.register(4242, "curate-10", sid)
            fx.register(1111, "schedule-10", rs.daily_session_id("plan", DATE, 0))
            os.environ[pc.KIND_ENV] = "curate"
            rc, killed = fx.cycle_cli()
            state = fx.repo / rs.STATE_DIR
            marker = (state / "curate-cycle-pending").read_text().strip()
            self.assertFalse((state / pc.MARKER).exists())
        self.assertEqual(rc, 0)
        self.assertEqual(marker, sid)
        self.assertEqual(killed, [4242])

    def test_the_curator_snoozes_apart_from_the_scheduler(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx, priced():
            sid = rs.daily_session_id("curate", DATE, 0)
            path = fx.home / "projects" / rs.slugify(fx.repo) / f"{sid}.jsonl"
            path.parent.mkdir(parents=True)
            path.write_text(json.dumps(assistant(0, 10, 0, WORKER)) + "\n")
            saved = pc.snooze(fx.repo, DATE, "curate")
            state = fx.repo / rs.STATE_DIR
            self.assertTrue((state / pc.snooze_name("curate")).is_file())
            self.assertFalse((state / pc.SNOOZE).exists())
        self.assertEqual(saved["sid"], sid)

    def test_the_curators_check_reads_its_own_snooze_only(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx, priced():
            sid = rs.daily_session_id("curate", DATE, 0)
            path = fx.home / "projects" / rs.slugify(fx.repo) / f"{sid}.jsonl"
            path.parent.mkdir(parents=True)
            path.write_text(json.dumps(assistant(0, 10, 0, WORKER)) + "\n")
            state = fx.repo / rs.STATE_DIR
            state.mkdir(parents=True, exist_ok=True)
            # A scheduler snooze naming the curator's sid must not leak across.
            (state / pc.SNOOZE).write_text(
                json.dumps({"sid": sid, "usd": 99.0, "tokens": 9_999_999}) + "\n"
            )
            self.assertEqual(
                pc.check(fx.repo, DATE, "curate")["token_threshold"],
                pc.TOKEN_THRESHOLD,
            )
            saved = pc.snooze(fx.repo, DATE, "curate")
            got = pc.check(fx.repo, DATE, "curate")
        self.assertEqual(got["token_threshold"], saved["tokens"])
        self.assertGreater(got["token_threshold"], pc.TOKEN_THRESHOLD)

    def test_the_kind_flag_wins_over_the_env(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            os.environ[pc.KIND_ENV] = "plan"
            sid = rs.daily_session_id("curate", DATE, 0)
            fx.register(4242, "curate-10", sid)
            sent = []
            with mock.patch.object(pc.os, "kill", lambda pid, sig: sent.append(pid)):
                with redirect_stdout(io.StringIO()):
                    rc = pc.run(
                        [
                            "plan_cycle.py",
                            "--repo",
                            str(fx.repo),
                            "--date",
                            DATE,
                            "--kind",
                            "curate",
                            "cycle",
                        ]
                    )
            marker = fx.repo / rs.STATE_DIR / pc.marker_name("curate")
            self.assertEqual(marker.read_text().strip(), sid)
        self.assertEqual(rc, 0)
        self.assertIn(4242, sent)

    def test_an_unknown_env_kind_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            os.environ[pc.KIND_ENV] = "architect"
            with self.assertRaises(pc.PlanCycleError):
                pc.run(["plan_cycle.py", "--repo", str(fx.repo), "check"])
            # live-name is name-keyed, so the stray kind cannot block the guard.
            self.assertEqual(pc.run(["plan_cycle.py", "live-name", "curate-10"]), 1)


def _probe(pid: int) -> None:
    if pid != os.getpid():
        raise ProcessLookupError(pid)


if __name__ == "__main__":
    unittest.main()
