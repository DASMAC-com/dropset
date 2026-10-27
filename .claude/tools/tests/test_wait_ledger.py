"""Tests for ``wait_ledger.py`` — the per-session wait ledger and the
digest's wait section, over transcript, fleet-log and launch-record
fixtures."""

from __future__ import annotations

import json
import pathlib
import tempfile
import unittest

import wait_ledger as wl


def stamp(minute: int, hour: int = 10) -> str:
    return f"2026-10-27T{hour:02d}:{minute:02d}:00"


def assistant(at: str, *uses: dict) -> dict:
    content = [{"type": "text", "text": "ok"}, *uses]
    return {
        "type": "assistant",
        "timestamp": at,
        "message": {"role": "assistant", "content": content},
    }


def use(tid: str, name: str, **args) -> dict:
    return {"type": "tool_use", "id": tid, "name": name, "input": args}


def result(at: str, tid: str, text: str = "done") -> dict:
    item = {"type": "tool_result", "tool_use_id": tid, "content": text}
    return {
        "type": "user",
        "timestamp": at,
        "message": {"role": "user", "content": [item]},
    }


def typed(at: str, text: str) -> dict:
    return {
        "type": "user",
        "timestamp": at,
        "message": {"role": "user", "content": text},
    }


def ledger(records: list[dict], since=None, until=None) -> dict[str, wl.DimensionLine]:
    w = wl.WaitLedger(since, until)
    for rec in records:
        w.observe(rec, rec["timestamp"])
    return {line.dimension: line for line in w.finish()}


LOCKED = f"    {wl.LOCK_MARKER} on build directory\n   Compiling dropset"


class Shapes(unittest.TestCase):
    def test_lock_steps(self):
        cases = {
            "make lint": "make-lint",
            "make test": "make-test",
            "make -j4 test-parity": "make-test-parity",
            "python3 .claude/tools/run_quiet.py -- make test": "make-test",
            "python3 .claude/tools/run_quiet.py -- python3 .claude/tools/lint_paths.py --changed": "lint-changed",
            "cargo clippy --workspace": "cargo-clippy",
            "CARGO_TERM_COLOR=never cargo +nightly doc": "cargo-doc",
        }
        for command, want in cases.items():
            self.assertEqual(wl.lock_step(command), want, command)

    def test_watchers_by_shape(self):
        self.assertEqual(
            wl.bash_wait("python3 .claude/tools/wait_for_checks.py --pr 654"),
            (wl.CHECKS, ""),
        )
        self.assertEqual(
            wl.bash_wait("python3 .claude/tools/wait_for_checks.py --queue 654"),
            (wl.QUEUE, ""),
        )
        self.assertEqual(wl.bash_wait("gh pr checks 654 --watch"), (wl.CHECKS, ""))
        self.assertEqual(
            wl.bash_wait("python3 .claude/tools/verify_head.py"), (wl.RACE, "")
        )
        self.assertIsNone(wl.bash_wait("make lint"))


class Ledger(unittest.TestCase):
    def test_lock_wait_is_the_blocked_call_by_step(self):
        lines = ledger(
            [
                assistant(stamp(0), use("a", "Bash", command="make lint")),
                result(stamp(12), "a", LOCKED),
                assistant(stamp(13), use("b", "Bash", command="cargo clippy")),
                result(stamp(16), "b", LOCKED),
                assistant(stamp(17), use("c", "Bash", command="make test")),
                result(stamp(40), "c", "test result: ok"),
            ]
        )
        lock = lines[wl.LOCK]
        self.assertEqual((lock.seconds, lock.calls), (15 * 60, 2))
        self.assertEqual(lock.causes, {"make-lint": 720, "cargo-clippy": 180})

    def test_a_quiet_wrapped_build_still_reads_as_locked(self):
        # The house form: `run_quiet.py` leaves one summary line, not cargo's.
        command = "python3 .claude/tools/run_quiet.py -- make test"
        summary = "✓ make test (exit 0, 900 lines; log: /t/x.log) — waited on a cargo file lock"
        lines = ledger(
            [
                assistant(stamp(0), use("a", "Bash", command=command)),
                result(stamp(6), "a", summary),
            ]
        )
        self.assertEqual(lines[wl.LOCK].causes, {"make-test": 360})

    def test_the_quiet_runner_marker_is_run_quiets_own_text(self):
        source = (pathlib.Path(wl.__file__).parent / "run_quiet.py").read_text("utf-8")
        self.assertIn(wl.LOCK_MARKERS[1], source)

    def test_ci_checks_queue_and_a_backgrounded_watch(self):
        lines = ledger(
            [
                assistant(
                    stamp(0),
                    use(
                        "a",
                        "Bash",
                        command="python3 .claude/tools/wait_for_checks.py --pr 9",
                    ),
                ),
                result(stamp(7), "a"),
                assistant(
                    stamp(8),
                    use(
                        "b",
                        "Bash",
                        command="python3 .claude/tools/wait_for_checks.py --queue 9",
                        run_in_background=True,
                    ),
                ),
                result(stamp(8), "b", "Command running in background with ID: bq42x"),
                typed(
                    stamp(30),
                    "<task-notification><task-id>bq42x</task-id></task-notification>",
                ),
            ]
        )
        self.assertEqual(lines[wl.CHECKS].seconds, 7 * 60)
        self.assertEqual(lines[wl.QUEUE].seconds, 22 * 60)

    def test_race_counts_the_settling_side(self):
        lines = ledger(
            [
                assistant(
                    stamp(0),
                    use("a", "Bash", command="python3 .claude/tools/verify_head.py"),
                ),
                result(
                    stamp(5),
                    "a",
                    "lint: ci green job=Lint; test: local red log=/tmp/t.log; sha abc",
                ),
            ]
        )
        race = lines[wl.RACE]
        self.assertEqual((race.seconds, race.sides), (300, {"ci": 1, "local": 1}))

    def test_operator_by_skill_point_and_a_refused_permission(self):
        lines = ledger(
            [
                typed(stamp(0), "<command-name>/review-pr</command-name>"),
                assistant(
                    stamp(1),
                    use(
                        "a",
                        "AskUserQuestion",
                        questions=[{"header": "Enqueue", "question": "?"}],
                    ),
                ),
                result(stamp(21), "a", "User answered: yes"),
                assistant(stamp(22), use("b", "Bash", command="git push --force")),
                result(
                    stamp(25),
                    "b",
                    "The user doesn't want to proceed with this tool use.",
                ),
            ]
        )
        op = lines[wl.OPERATOR]
        self.assertEqual(
            op.causes, {"review-pr-enqueue": 1200, "review-pr-permission": 180}
        )

    def test_idle_only_while_nothing_is_pending(self):
        lines = ledger(
            [
                assistant(stamp(0)),
                typed(stamp(45), "next please"),
                assistant(
                    stamp(46),
                    use(
                        "a",
                        "Bash",
                        command="python3 .claude/tools/wait_for_checks.py --pr 9",
                        run_in_background=True,
                    ),
                ),
                result(stamp(46), "a", "Command running in background with ID: zz9"),
                assistant(stamp(47)),
                # The operator speaks while the watch is still running: not idle.
                typed(stamp(55), "status?"),
                typed(
                    stamp(58),
                    "<task-notification><task-id>zz9</task-id> done</task-notification>",
                ),
            ]
        )
        self.assertEqual((lines[wl.IDLE].seconds, lines[wl.IDLE].calls), (45 * 60, 1))
        self.assertEqual(lines[wl.CHECKS].seconds, 12 * 60)

    def test_subagents_listed_last_and_ranking(self):
        w = wl.WaitLedger()
        for rec in [
            assistant(stamp(0), use("a", "Agent", subagent_type="Explore", prompt="x")),
            result(stamp(50), "a", "report"),
            assistant(stamp(51), use("b", "Bash", command="make lint")),
            result(stamp(52), "b", LOCKED),
            typed(stamp(59), "hi"),
        ]:
            w.observe(rec, rec["timestamp"])
        order = [line.dimension for line in w.finish()]
        self.assertEqual(order[:2], [wl.IDLE, wl.LOCK])
        self.assertEqual(order[-1], wl.SUBAGENTS)
        self.assertEqual(len(order), len(wl.DIMENSIONS))
        text = "\n".join(wl.ledger_lines(w.finish()))
        self.assertIn(
            "sub-agents: 50m over 1 wait(s) — explore 50m (listed, not counted", text
        )
        self.assertIn("verify race: none", text)
        self.assertIn("Dominated by idle: a session that should have closed.", text)

    def test_a_synchronous_agent_is_not_parked(self):
        # Its result carries an `agentId:` trailer, but no notification ever
        # follows: parked, it would never count and would block idle for good.
        lines = ledger(
            [
                assistant(stamp(0), use("a", "Agent", subagent_type="Explore")),
                result(stamp(20), "a", "report\nagentId: abc123 (for resuming)"),
                assistant(stamp(21)),
                typed(stamp(51), "thanks"),
            ]
        )
        self.assertEqual(lines[wl.SUBAGENTS].seconds, 20 * 60)
        self.assertEqual(lines[wl.IDLE].seconds, 30 * 60)

    def test_an_async_agent_closes_on_its_task_id_only(self):
        lines = ledger(
            [
                assistant(stamp(0), use("a", "Agent", subagent_type="Explore")),
                result(
                    stamp(0), "a", "Async agent launched successfully.\nagentId: abc123"
                ),
                # Naming the id in passing is not its completion.
                typed(
                    stamp(5),
                    "<task-notification><task-id>zzz</task-id> abc123</task-notification>",
                ),
                typed(
                    stamp(9),
                    "<task-notification><task-id>abc123</task-id></task-notification>",
                ),
            ]
        )
        self.assertEqual(lines[wl.SUBAGENTS].seconds, 9 * 60)

    def test_a_notification_queued_while_busy_closes_the_wait(self):
        queued = {
            "type": "attachment",
            "timestamp": stamp(30),
            "attachment": {
                "type": "queued_command",
                "prompt": "<task-notification>\n<task-id>abc123</task-id>",
            },
        }
        lines = ledger(
            [
                assistant(stamp(0), use("a", "Agent", subagent_type="Explore")),
                result(
                    stamp(0), "a", "Async agent launched successfully.\nagentId: abc123"
                ),
                queued,
                assistant(stamp(31)),
                typed(stamp(41), "ok"),
            ]
        )
        self.assertEqual(lines[wl.SUBAGENTS].seconds, 30 * 60)
        self.assertEqual(lines[wl.IDLE].seconds, 10 * 60)

    def test_a_refusal_and_an_interrupt_are_not_idle_too(self):
        lines = ledger(
            [
                assistant(stamp(0), use("a", "Bash", command="git push --force")),
                result(
                    stamp(10),
                    "a",
                    "The user doesn't want to proceed with this tool use.",
                ),
                typed(stamp(12), "[Request interrupted by user for tool use]"),
                typed(stamp(14), "use a lease"),
            ]
        )
        self.assertEqual(lines[wl.OPERATOR].seconds, 10 * 60)
        self.assertEqual(lines[wl.IDLE].seconds, 4 * 60)

    def test_window_keeps_waits_that_start_inside(self):
        records = [
            assistant(stamp(0), use("a", "Bash", command="make lint")),
            result(stamp(10), "a", LOCKED),
            assistant(stamp(20), use("b", "Bash", command="make test")),
            result(stamp(30), "b", LOCKED),
        ]
        lines = ledger(records, since=stamp(15), until=stamp(59))
        self.assertEqual(lines[wl.LOCK].causes, {"make-test": 600})

    def test_a_repeated_tool_use_counts_once(self):
        call = use("a", "Bash", command="make lint")
        lines = ledger(
            [
                assistant(stamp(0), call),
                assistant(stamp(0), call),
                result(stamp(4), "a", LOCKED),
            ]
        )
        self.assertEqual(lines[wl.LOCK].calls, 1)


class SessionReport(unittest.TestCase):
    def test_the_report_carries_the_ledger_in_both_renderings(self):
        import session_metrics as sm

        agg = sm.SessionAggregator()
        for rec in [
            assistant(stamp(0) + "Z", use("a", "Bash", command="make lint")),
            result(stamp(9) + "Z", "a", LOCKED),
        ]:
            agg.ingest_main_line(json.dumps(rec))
        report = agg.finish(sm.SUBSTRATE_BEDROCK, "abc")
        self.assertIn(
            "- pool lock: 9m over 1 wait(s) — make-lint 9m",
            sm.to_markdown(report, "abc"),
        )
        first = json.loads(sm.to_json(report))["wait"][0]
        self.assertEqual((first["dimension"], first["seconds"]), (wl.LOCK, 540))


FLEET_LOG = """\
2026-10-27T09:59:00Z builders=1 builds=0 waiting=- longest_s=0 in_use_gib=20.0
2026-10-27T10:05:00Z builders=3 builds=1 waiting=clippy:1,test:2 longest_s=140 in_use_gib=31.5
2026-10-27T10:40:00Z builders=2 builds=1 waiting=test:1 longest_s=410 in_use_gib=29.0
2026-10-27T11:10:00Z builders=1 builds=1 waiting=- longest_s=0 in_use_gib=22.0
not a sample line
"""


class Digest(unittest.TestCase):
    def test_queue_profile_by_hour(self):
        profile = wl.queue_profile(FLEET_LOG, stamp(0), stamp(0, hour=12))
        self.assertEqual(list(profile), ["2026-10-27T10", "2026-10-27T11"])
        self.assertEqual(
            profile["2026-10-27T10"],
            {"samples": 2, "depth": 3, "longest_s": 410, "in_use_gib": 31.5},
        )

    def test_load_records_in_window(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp, "abc.load")
            path.write_text(
                "\n".join(
                    [
                        json.dumps({"at": "2026-10-26T23:00:00Z", "class": "free"}),
                        json.dumps({"at": "2026-10-27T10:01:00Z", "class": "building"}),
                        "{torn",
                    ]
                )
            )
            got = wl.read_load_records(pathlib.Path(tmp), stamp(0), stamp(0, hour=12))
        self.assertEqual([r["class"] for r in got], ["building"])

    def test_section_ranks_causes_with_fingerprints(self):
        def session(lock_s: int, idle_s: int, checks_s: int) -> list[wl.DimensionLine]:
            return [
                wl.DimensionLine(wl.LOCK, lock_s, 1, {"make-lint": lock_s}),
                wl.DimensionLine(wl.IDLE, idle_s, 1, {"": idle_s}),
                wl.DimensionLine(wl.CHECKS, checks_s, 1, {"": checks_s}),
                wl.DimensionLine(wl.SUBAGENTS, 9000, 1, {"explore": 9000}),
            ]

        loads = [
            {
                "longest_wait_s": 100,
                "building": 3,
                "free": 1,
                "in_use": 30 * 1024**3,
                "verdict": {"reasons": ["memory"]},
            },
            {
                "longest_wait_s": 300,
                "building": 5,
                "free": 1,
                "in_use": 20 * 1024**3,
                "verdict": {"reasons": []},
            },
        ]
        ci = {"critical_path": [["Tests (Rust)", 4], ["Lint", 1]]}
        lines, twin = wl.digest_section(
            [session(3600, 600, 1800), session(1800, 0, 600)],
            loads,
            wl.queue_profile(FLEET_LOG, stamp(0), stamp(0, hour=12)),
            ci,
        )
        text = "\n".join(lines)
        self.assertTrue(
            lines[0].startswith(
                "**Daily**: pool lock 1.50h, CI: checks 0.67h, idle 0.17h"
            )
        )
        self.assertIn("Fleet build wait by target: make-lint 1.50h", text)
        self.assertIn("CI wait: 0.67h (checks 0.67h, queue 0.00h)", text)
        self.assertIn("10h 3/410s, 11h 0/0s", text)
        self.assertIn(
            "median longest wait at launch 200s; median builders 4, free 1;"
            " memory refusals 1; peak in_use 31.5 GiB",
            text,
        )
        self.assertIn(
            "pool lock · make-lint: 1.50h over 2 session(s), median 45m per session"
            " — `**Fingerprint**: wait:lock:make-lint`",
            text,
        )
        self.assertIn("wait:ci-checks:tests-rust", text)
        self.assertIn(
            "idle · unnamed: 0.17h over 1 session(s), median 10m per session"
            " — no nameable cause — report, don't file",
            text,
        )
        self.assertNotIn("explore", text)
        self.assertEqual(twin["ranked"], [wl.LOCK, wl.CHECKS, wl.IDLE])
        self.assertEqual(twin["causes"][0]["fingerprint"], "wait:lock:make-lint")
        self.assertEqual(twin["memory_refusals"], 1)

    def test_ci_without_a_read_is_not_filed(self):
        lines, twin = wl.digest_section(
            [[wl.DimensionLine(wl.QUEUE, 1200, 1, {"": 1200})]], [], {}, {"error": "x"}
        )
        self.assertIn("no nameable cause", "\n".join(lines))
        self.assertIsNone(twin["causes"][0]["fingerprint"])

    def test_an_empty_day(self):
        lines, _ = wl.digest_section([], [], {}, None)
        self.assertEqual(lines[0], "**Daily**: no waiting measured")
        self.assertIn("- nothing to rank", lines)


if __name__ == "__main__":
    unittest.main()
