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
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import plan_cycle as pc  # noqa: E402
import resolve_session as rs  # noqa: E402

DATE = "20260910"


def assistant(inp: int, read: int, write: int) -> dict:
    return {
        "type": "assistant",
        "message": {
            "usage": {
                "input_tokens": inp,
                "cache_read_input_tokens": read,
                "cache_creation_input_tokens": write,
                "output_tokens": 999,
            }
        },
    }


#: The record shape a received cross-session message takes in a transcript.
PEER = {
    "type": "user",
    "isMeta": True,
    "origin": {"kind": "peer", "name": "eng-1599"},
    "message": {"content": "<cross-session-message ...>"},
}

#: The same text echoed by an attachment and a queue record, which must not
#: count — the substring alone appears three times per message.
ECHOES = [
    {"type": "attachment", "content": '"kind": "peer"'},
    {"type": "queue-operation", "content": '"kind": "peer"'},
]


class Fixture:
    def __init__(self, tmp: str):
        self.root = Path(tmp)
        self.repo = self.root / "repo"
        self.home = self.root / "claude"
        self.repo.mkdir()
        self.env = mock.patch.dict(os.environ, {"CLAUDE_CONFIG_DIR": str(self.home)})

    def __enter__(self):
        self.env.start()
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

    def register(self, pid: int, name: str, sid: str) -> None:
        sessions = self.home / "sessions"
        sessions.mkdir(parents=True, exist_ok=True)
        (sessions / f"{pid}.json").write_text(
            json.dumps({"pid": pid, "name": name, "sessionId": sid})
        )


class CheckTests(unittest.TestCase):
    def test_prefix_is_the_last_assistant_input_side(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(1, 10, 100), PEER, assistant(2, 200, 3000)])
            got = pc.check(fx.repo, DATE)
        self.assertEqual(got["prefix_tokens"], 3202)
        self.assertEqual(got["inbound_messages"], 1)
        self.assertFalse(got["due"])

    def test_echo_records_are_not_counted_as_messages(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([*ECHOES, PEER, *ECHOES, PEER, assistant(0, 0, 0)])
            self.assertEqual(pc.check(fx.repo, DATE)["inbound_messages"], 2)

    def test_due_on_tokens_alone(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(0, pc.TOKEN_THRESHOLD, 0)])
            self.assertTrue(pc.check(fx.repo, DATE)["due"])

    def test_due_on_messages_alone(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([PEER] * pc.MESSAGE_THRESHOLD + [assistant(0, 1, 0)])
            self.assertTrue(pc.check(fx.repo, DATE)["due"])

    def test_the_tail_read_skips_the_partial_first_line(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            filler = {"type": "user", "message": {"content": "x" * 4096}}
            fx.transcript([assistant(0, 7, 0)] + [filler] * 400)
            with mock.patch.object(pc, "TAIL_BYTES", 10_000):
                # No usage in the tail: the whole-file fallback finds it.
                self.assertEqual(pc.check(fx.repo, DATE)["prefix_tokens"], 7)

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
    def test_snooze_moves_both_thresholds_past_the_current_values(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            fx.transcript([assistant(0, 260_000, 0)])
            pc.snooze(fx.repo, DATE)
            got = pc.check(fx.repo, DATE)
        self.assertEqual(got["token_threshold"], 260_000 + pc.SNOOZE_TOKENS)
        self.assertEqual(
            got["message_threshold"], pc.MESSAGE_THRESHOLD + pc.SNOOZE_MESSAGES
        )
        self.assertFalse(got["due"])

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

    def test_cycle_writes_the_marker_and_signals_its_own_client(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            sid = rs.daily_session_id("plan", DATE, 0)
            fx.register(4242, "plan-10", sid)
            fx.register(4343, "eng-1", "other")
            sent = []
            with mock.patch.object(
                pc.os, "kill", lambda pid, sig: sent.append((pid, sig))
            ):
                out = io.StringIO()
                with redirect_stdout(out):
                    rc = pc.run(
                        [
                            "plan_cycle.py",
                            "--repo",
                            str(fx.repo),
                            "--date",
                            DATE,
                            "cycle",
                        ]
                    )
            marker = (fx.repo / rs.STATE_DIR / pc.MARKER).read_text().strip()
        self.assertEqual(rc, 0)
        self.assertEqual(marker, sid)
        self.assertIn((4242, pc.signal.SIGTERM), sent)
        self.assertNotIn(4343, [pid for pid, sig in sent if sig])

    def test_cycle_with_no_client_still_leaves_the_marker(self):
        with tempfile.TemporaryDirectory() as tmp, Fixture(tmp) as fx:
            with self.assertRaises(pc.PlanCycleError):
                pc.cycle(fx.repo, DATE)
            self.assertTrue((fx.repo / rs.STATE_DIR / pc.MARKER).is_file())


def _probe(pid: int) -> None:
    if pid != os.getpid():
        raise ProcessLookupError(pid)


if __name__ == "__main__":
    unittest.main()
