#!/usr/bin/env python3
"""Behavior tests for the iTerm attention signal (stdlib unittest).

Two operator-facing rules, both driven against the real shell, for the reason
`test_iterm_heal.py` gives: the painter and the monitor stay bash, so the tests
run them in a subprocess rather than porting the logic to have something
importable to test.

* **The debounce.** A yellow is armed by the painter and only promoted by the
  monitor once it has outlived `PERM_DEBOUNCE_SECONDS`, so a prompt the auto
  classifier answers by itself never flashes the tab or moves it.
* **Self-waiting is quiet.** A `Stop` with a background task or a scheduled
  wake-up in flight paints the slate rather than green, and the notifications
  this integration owns go out only for the needs-operator class.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
COLORS = SCRIPTS / "iterm-colors.sh"
PAINT = SCRIPTS / "iterm-paint.sh"

# The promotion reads the arm's mtime with BSD `stat -f %m`; see
# test_iterm_heal.py for why that is gated rather than ported.
_DARWIN_ONLY = "iTerm2 tab coloring is macOS-only (BSD `stat -f %m`)"

# Kept in step with iterm-colors.sh deliberately: a test that read the palette
# out of the file under test could not catch the palette changing.
STATE_PERMISSION = "3a2c08"
STATE_NEUTRAL = "16191e"
STATE_REPLY = "080c2a"
STATE_WAITING = "141c26"

OSC9 = "\x1b]9;"


class Harness(unittest.TestCase):
    """A temp STATE_PREFIX, a plain file standing in for the tty, and an env
    stripped of the operator's own overrides."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        root = Path(self._tmp.name)
        self.prefix = str(root / "iterm-color-")
        self.tty_base = "ttys999"
        self.tty = root / self.tty_base
        self.tty.write_text("", encoding="utf-8")
        self.state = Path(f"{self.prefix}{self.tty_base}")
        self.pending = Path(f"{self.prefix}{self.tty_base}.permpending")
        self.askq = Path(f"{self.prefix}{self.tty_base}.askq")
        self.env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith("ITERM_") and k != "CLAUDE_CONFIG_FILE"
        }
        self.env["ITERM_TTY_PATH"] = str(self.tty)
        # Never a live session's state or pid files, nor the real ~/.claude.json.
        self.env["ITERM_STATE_PREFIX"] = self.prefix
        self.pid_prefix = str(root / "iterm-monitor-")
        self.env["ITERM_MONITOR_PID_PREFIX"] = self.pid_prefix
        self.env["CLAUDE_CONFIG_FILE"] = str(root / "claude.json")

    def _prefixed(self, body: str) -> str:
        return f'source "{COLORS}"; {body}'

    def paint(self, payload: dict, *, notify: bool = False):
        """Run the real painter in hook mode on `payload`."""
        env = {**self.env, "ITERM_PAINTER_NOTIFY": "1" if notify else "0"}
        return subprocess.run(
            ["bash", str(PAINT)],
            input=json.dumps(payload),
            capture_output=True,
            text=True,
            check=False,
            env=env,
        )

    def shell(self, body: str, **extra):
        return subprocess.run(
            ["bash", "-c", self._prefixed(body)],
            capture_output=True,
            text=True,
            check=False,
            env={**self.env, **extra},
        )

    def read(self, path: Path) -> str:
        return path.read_text(encoding="utf-8").strip() if path.exists() else ""

    def tty_output(self) -> str:
        return self.tty.read_text(encoding="utf-8")

    def age(self, path: Path, seconds: float):
        stamp = time.time() - seconds
        os.utime(path, (stamp, stamp))


def _permission(message="Claude needs your permission to use Bash"):
    return {
        "hook_event_name": "Notification",
        "notification_type": "permission_prompt",
        "message": message,
    }


class TheDebounceArms(Harness):
    def setUp(self):
        super().setUp()
        self.state.write_text(STATE_NEUTRAL, encoding="utf-8")

    def test_a_permission_prompt_arms_without_painting(self):
        """The flicker fix: nothing the reorderer reads changes yet."""
        result = self.paint(_permission("go approve"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.read(self.state), STATE_NEUTRAL)
        self.assertEqual(self.pending.read_text(encoding="utf-8"), "go approve")
        self.assertEqual(self.tty_output(), "", "no SetColors may be emitted")

    def test_an_edit_tool_yellow_arms_too(self):
        self.paint({"hook_event_name": "PreToolUse", "tool_name": "Write"})
        self.assertEqual(self.read(self.state), STATE_NEUTRAL)
        self.assertTrue(self.pending.exists())

    def test_any_other_paint_cancels_the_arm(self):
        """The classifier approved: the tool ran and its PostToolUse fires."""
        self.paint(_permission())
        self.paint({"hook_event_name": "PostToolUse", "tool_name": "Bash"})
        self.assertFalse(self.pending.exists())
        self.assertEqual(self.read(self.state), STATE_NEUTRAL)

    def test_a_re_fire_keeps_the_original_timestamp(self):
        """Otherwise the harness's periodic re-fire would restart the debounce
        for ever and a genuine prompt would never surface."""
        self.paint(_permission())
        self.age(self.pending, 30)
        before = self.pending.stat().st_mtime
        self.paint(_permission())
        self.assertEqual(self.pending.stat().st_mtime, before)

    def test_an_edit_then_its_prompt_gains_the_message_but_keeps_the_time(self):
        """The common order for an edit that needs approval."""
        self.paint({"hook_event_name": "PreToolUse", "tool_name": "Edit"})
        self.age(self.pending, 30)
        before = self.pending.stat().st_mtime
        self.paint(_permission("approve the edit"))
        self.assertEqual(self.pending.read_text(encoding="utf-8"), "approve the edit")
        self.assertEqual(self.pending.stat().st_mtime, before)

    def test_a_yellow_already_shown_is_not_re_armed(self):
        self.state.write_text(STATE_PERMISSION, encoding="utf-8")
        self.paint(_permission())
        self.assertFalse(self.pending.exists())


@unittest.skipUnless(sys.platform == "darwin", _DARWIN_ONLY)
class TheMonitorPromotes(Harness):
    def setUp(self):
        super().setUp()
        self.state.write_text(STATE_NEUTRAL, encoding="utf-8")

    def promote(self, *, debounce="5", notify="0", now: int | None = None):
        # `now` pins the clock by shadowing `date`, so a boundary test measures
        # exactly the age it sets rather than whatever second it lands in.
        clock = f"date() {{ echo {now}; }}; " if now is not None else ""
        return self.shell(
            f'{clock}PERM_DEBOUNCE_SECONDS="{debounce}"; '
            f'promote_pending_permission "{self.tty_base}" "{self.tty}"',
            ITERM_PAINTER_NOTIFY=notify,
        )

    def _arm_at(self, stamp: int, message=""):
        self.pending.write_text(message, encoding="utf-8")
        os.utime(self.pending, (stamp, stamp))

    def test_an_arm_that_outlived_the_debounce_turns_yellow(self):
        self.pending.write_text("", encoding="utf-8")
        self.age(self.pending, 6)
        result = self.promote()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.read(self.state), STATE_PERMISSION)
        self.assertFalse(self.pending.exists())

    def test_the_boundary_promotes_at_exactly_the_debounce(self):
        """`-ge`, not `-gt`, pinned on a fixed clock."""
        stamp = int(time.time()) - 100
        self._arm_at(stamp)
        result = self.promote(debounce="5", now=stamp + 5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.read(self.state), STATE_PERMISSION)

    def test_one_second_short_of_the_debounce_waits(self):
        stamp = int(time.time()) - 100
        self._arm_at(stamp)
        self.promote(debounce="5", now=stamp + 4)
        self.assertEqual(self.read(self.state), STATE_NEUTRAL)
        self.assertTrue(self.pending.exists())

    def test_a_fresh_arm_waits(self):
        self.pending.write_text("", encoding="utf-8")
        self.promote()
        self.assertEqual(self.read(self.state), STATE_NEUTRAL)
        self.assertTrue(self.pending.exists())

    def test_promotion_delivers_the_armed_notification(self):
        self.pending.write_text("go approve", encoding="utf-8")
        self.age(self.pending, 6)
        self.promote(notify="1")
        self.assertEqual(self.tty_output(), f"{OSC9}go approve\a")
        self.assertEqual(self.read(self.state), STATE_PERMISSION)
        self.assertFalse(self.pending.exists())

    def test_an_edit_arm_promotes_silently(self):
        # The harness never notified for the edit-tool yellow either. The
        # state assertion is what stops a promotion that did nothing passing.
        self.pending.write_text("", encoding="utf-8")
        self.age(self.pending, 6)
        self.promote(notify="1")
        self.assertEqual(self.tty_output(), "")
        self.assertEqual(self.read(self.state), STATE_PERMISSION)


class StopIsQuietWhileSelfWaiting(Harness):
    def _stop(self, **fields):
        self.paint({"hook_event_name": "Stop", "last_assistant_message": "", **fields})
        return self.read(self.state)

    def test_a_background_task_paints_the_slate(self):
        self.assertEqual(self._stop(background_tasks=[{"id": "b1"}]), STATE_WAITING)

    def test_a_scheduled_wake_up_paints_the_slate(self):
        self.assertEqual(self._stop(session_crons=[{"id": "c1"}]), STATE_WAITING)

    def test_nothing_in_flight_is_green(self):
        self.assertEqual(self._stop(background_tasks=[], session_crons=[]), STATE_REPLY)

    def test_an_older_harness_without_the_fields_is_green(self):
        """The failure direction: silencing a session that needed a reply is
        worse than a green that did not."""
        self.assertEqual(self._stop(), STATE_REPLY)

    def test_the_field_name_quoted_inside_the_message_does_not_count(self):
        message = 'see "background_tasks": ["x"] in the payload'
        self.assertEqual(
            self._stop(last_assistant_message=message, background_tasks=[]),
            STATE_REPLY,
        )


class OnlyTheNeedsOperatorClassNotifies(Harness):
    def _idle(self):
        return {
            "hook_event_name": "Notification",
            "notification_type": "idle_prompt",
            "message": "Claude is waiting for your input",
        }

    def test_idle_on_a_self_waiting_tab_is_silent(self):
        self.state.write_text(STATE_WAITING, encoding="utf-8")
        self.paint(self._idle(), notify=True)
        self.assertEqual(self.tty_output(), "")

    def test_idle_on_a_reply_tab_notifies(self):
        self.state.write_text(STATE_REPLY, encoding="utf-8")
        self.paint(self._idle(), notify=True)
        self.assertEqual(self.tty_output(), f"{OSC9}Claude is waiting for your input\a")

    def test_idle_does_not_repaint(self):
        self.state.write_text(STATE_WAITING, encoding="utf-8")
        self.paint(self._idle(), notify=True)
        self.assertEqual(self.read(self.state), STATE_WAITING)

    def test_a_non_operator_type_is_silent(self):
        self.paint(
            {
                "hook_event_name": "Notification",
                "notification_type": "agent_completed",
                "message": "done",
            },
            notify=True,
        )
        self.assertEqual(self.tty_output(), "")

    def test_nothing_is_sent_while_the_harness_owns_notifications(self):
        self.state.write_text(STATE_REPLY, encoding="utf-8")
        self.paint(self._idle(), notify=False)
        self.assertEqual(self.tty_output(), "")

    def test_an_askq_selector_notifies_once_across_re_fires(self):
        self.paint({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion"})
        self.tty.write_text("", encoding="utf-8")  # drop the green's SetColors
        self.paint(_permission("pick one"), notify=True)
        self.paint(_permission("pick one"), notify=True)
        self.assertEqual(self.tty_output(), f"{OSC9}pick one\a")
        self.assertEqual(self.read(self.state), STATE_REPLY)
        self.assertFalse(self.pending.exists(), "the companion must not arm a yellow")


class NotificationOwnership(Harness):
    def owns(self, config: str | None, **extra) -> bool:
        if config is not None:
            Path(self.env["CLAUDE_CONFIG_FILE"]).write_text(config, encoding="utf-8")
        return self.shell("painter_owns_notifications", **extra).returncode == 0

    def test_the_harness_channel_switched_off_hands_it_over(self):
        self.assertTrue(
            self.owns('{"preferredNotifChannel": "notifications_disabled"}')
        )

    def test_any_other_channel_leaves_it_with_the_harness(self):
        self.assertFalse(self.owns('{"preferredNotifChannel": "iterm2"}'))

    def test_no_config_leaves_it_with_the_harness(self):
        self.assertFalse(self.owns(None))

    def test_the_override_wins_both_ways(self):
        disabled = '{"preferredNotifChannel": "notifications_disabled"}'
        self.assertFalse(self.owns(disabled, ITERM_PAINTER_NOTIFY="0"))
        self.assertTrue(self.owns(None, ITERM_PAINTER_NOTIFY="1"))

    def test_control_characters_cannot_end_the_sequence_early(self):
        result = self.shell(
            "notify_operator \"$(printf 'a\\033b\\007c')\"", ITERM_PAINTER_NOTIFY="1"
        )
        self.assertEqual(result.stdout, f"{OSC9}abc\a")


class AMissingMonitorIsRestarted(Harness):
    """Only a monitor promotes an armed yellow, so a session without one would
    never surface a genuine prompt — which is why the painter, not just
    SessionStart, makes sure one is running. `/dev/null` stands in for the tty
    because the guard admits only a character device."""

    def setUp(self):
        super().setUp()
        self.fake_dir = Path(self._tmp.name) / "fake-scripts"
        self.fake_dir.mkdir()
        self.marker = Path(self._tmp.name) / "monitor-started"
        fake = self.fake_dir / "iterm-monitor.sh"
        fake.write_text(f'#!/bin/bash\necho "$1" >>"{self.marker}"\n', encoding="utf-8")
        fake.chmod(0o755)
        self.pid_file = Path(f"{self.pid_prefix}null.pid")

    def ensure(self, tty="/dev/null"):
        result = self.shell(f'SCRIPT_DIR="{self.fake_dir}"; ensure_monitor "{tty}"')
        self.assertEqual(result.returncode, 0, result.stderr)

    # How long an assert that a monitor DOES start waits for the backgrounded
    # spawn. A busy machine overran the old fixed 2s and read a slow spawn as a
    # missing one; the asserts that NONE starts keep the short default window.
    STARTS_WITHIN = 30.0

    def started(self, within: float = 2.0) -> bool:
        deadline = time.monotonic() + within
        while True:
            if self.marker.exists():
                return True
            if time.monotonic() >= deadline:
                return False
            time.sleep(0.05)

    def test_none_running_starts_one_and_records_its_pid(self):
        self.ensure()
        self.assertTrue(self.started(self.STARTS_WITHIN))
        self.assertTrue(self.read(self.pid_file).isdigit())

    def test_a_live_monitor_is_left_alone(self):
        self.pid_file.write_text(f"{os.getpid()}\n", encoding="utf-8")
        self.ensure()
        self.assertFalse(self.started())

    def test_a_dead_pid_is_replaced(self):
        dead = subprocess.run(["bash", "-c", "echo $$"], capture_output=True, text=True)
        self.pid_file.write_text(dead.stdout, encoding="utf-8")
        self.ensure()
        self.assertTrue(self.started(self.STARTS_WITHIN))

    def test_a_non_tty_gets_no_monitor(self):
        self.ensure(tty=str(self.tty))
        self.assertFalse(self.started())
        self.assertFalse(self.pid_file.exists())


class SessionStartReseeds(Harness):
    """SessionStart fires on `compact` too, which is the same session carrying
    on — so a reseed there would turn a green tab neutral mid-session."""

    def start(self, source: str):
        result = subprocess.run(
            ["bash", str(SCRIPTS / "iterm-start.sh")],
            input=json.dumps({"hook_event_name": "SessionStart", "source": source}),
            capture_output=True,
            text=True,
            check=False,
            env=self.env,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def setUp(self):
        super().setUp()
        self.state.write_text(STATE_REPLY, encoding="utf-8")
        self.pending.write_text("live arm", encoding="utf-8")

    def test_a_compact_keeps_the_state_and_the_arm(self):
        self.start("compact")
        self.assertEqual(self.read(self.state), STATE_REPLY)
        self.assertTrue(self.pending.exists())

    def test_a_resume_reseeds_and_drops_the_arm(self):
        self.start("resume")
        self.assertEqual(self.read(self.state), STATE_NEUTRAL)
        self.assertFalse(self.pending.exists())


class AnOrphanedMonitorExits(Harness):
    def test_a_monitor_the_pid_file_does_not_name_exits(self):
        """Two hooks racing through `ensure_monitor` can each start one; the
        loser, which no restart could find, must not poll for ever."""
        Path(f"{self.pid_prefix}null.pid").write_text("1\n", encoding="utf-8")
        result = subprocess.run(
            ["bash", str(SCRIPTS / "iterm-monitor.sh"), "/dev/null"],
            capture_output=True,
            text=True,
            check=False,
            env=self.env,
            timeout=60,
        )
        self.assertEqual(result.returncode, 0, result.stderr)


class DebounceDefault(unittest.TestCase):
    def test_the_default_is_5_and_is_overridable(self):
        override = "ITERM_PERM_DEBOUNCE_SECONDS"
        for env, want in (({}, "5"), ({override: "2"}, "2")):
            with self.subTest(env=env):
                child = {**os.environ}
                child.pop(override, None)
                child.update(env)
                result = subprocess.run(
                    [
                        "bash",
                        "-c",
                        f'source "{COLORS}"; printf "%s" "$PERM_DEBOUNCE_SECONDS"',
                    ],
                    capture_output=True,
                    text=True,
                    check=False,
                    env=child,
                )
                self.assertEqual(result.stdout.strip(), want)


if __name__ == "__main__":
    unittest.main()
