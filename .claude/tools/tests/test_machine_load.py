"""Stdlib ``unittest`` tests for the machine-load probe.

Every reading is fed from captured ``vm_stat``, ``sysctl`` and ``ps`` text
through a fake runner, so the verdict, each refusal reason for both classes,
the cargo parent-child classification and the no-reading exit are asserted
without touching this machine. Run via ``make tools-tests``.
"""

from __future__ import annotations

import io
import json
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import machine_load as ml

GIB = ml.GIB

# 16 KiB pages; anonymous 1,376,256 (21 GiB), wired 262,144 (4 GiB),
# compressor 655,360 (10 GiB): 35 GiB in use of 48.
VM_STAT = """\
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                              220209.
Pages wired down:                        262144.
"Translation faults":                5136724959.
File-backed pages:                       592492.
Anonymous pages:                        1376256.
Pages stored in compressor:             2230372.
Pages occupied by compressor:            655360.
"""


def sysctl(load="10.80", swap="0.00M", pressure=1):
    return (
        f"{{ {load} 12.78 12.21 }}\n18\n{48 * GIB}\n"
        f"total = 0.00M  used = {swap}  free = 0.00M  (encrypted)\n{pressure}\n"
    )


# A doc build with its rustdoc children; a cargo-clippy wrapper whose child
# cargo holds the lock queue (one invocation, not two); a bare test waiting
# 7m45s; a test running its own binary (building, not waiting); a cargo in
# its first seconds; a cargo stopped with ^Z and an idle cargo-watch (neither
# is waiting); two claude processes and the Docker VM.
PS = """\
    1     0  17360 07:33:36 Ss   launchd
  753     1 776384 07:33:11 S    com.apple.Virtualization.VirtualMachine
10069 80750  71312    10:24 S+   cargo
24399 10069 256352    00:26 R+   rustdoc
24425 10069 181296    00:26 R+   rustdoc
16048  6868  20288    03:25 S+   cargo-clippy
16112 16048  83120    03:25 S+   cargo
74477 98402  24560    07:45 S+   cargo
30000 98403  24560    01:00 S+   cargo
30001 30000  99999    00:40 R+   dropset-abc123
61255 38025  27184    00:03 S+   cargo
40000 98404  24560 02:00:00 T    cargo
40100 98405  24560 03:00:00 S+   cargo-watch
 3326  9309 403328 01:28:35 S+   claude
94579 81885 315040 1-01:26:41 S+   claude
"""

ARGS = """\
10069 /Users/x/.cargo/bin/cargo +stable doc --workspace --no-deps
16048 /Users/x/.cargo/bin/cargo-clippy clippy --all-targets
74477 cargo test -p dropset
30000 cargo test -p feeds
61255 cargo check
"""


def runner(vm=VM_STAT, ctl=None, ps=PS, args=ARGS):
    outputs = {"vm_stat": vm, "sysctl": ctl or sysctl()}

    def fake(cmd):
        if cmd[0] == "ps":
            return args if "-p" in cmd else ps
        return outputs[cmd[0]]

    return fake


def reading(**kw):
    return ml.read_machine(runner(**kw), state_dir=None)


#: The required knobs, as the operator's runtime config sets them; a test
#: unsets one by passing "".
FLEET = {"DS_FLEET_TOTAL": "16", "DS_FLEET_CPU_LOAD_MAX": "16"}


def knobs(r, **env):
    merged = {**FLEET, **{k: str(v) for k, v in env.items()}}
    return ml.read_knobs(merged, r.memory.physical)


class Parsing(unittest.TestCase):
    def test_memory_in_use_is_anonymous_wired_and_compressor(self):
        m = reading().memory
        self.assertEqual((m.app, m.wired, m.compressed), (21 * GIB, 4 * GIB, 10 * GIB))
        self.assertEqual(m.in_use, 35 * GIB)

    def test_sysctl_fields(self):
        s = ml.parse_sysctl(sysctl(load="16.70", swap="512.00M", pressure=2))
        self.assertEqual((s.load1, s.cores, s.physical), (16.7, 18, 48 * GIB))
        self.assertEqual((s.swap_used, s.pressure), (512 * 1024**2, 2))

    def test_etime_forms(self):
        self.assertEqual(ml.parse_etime("00:03"), 3)
        self.assertEqual(ml.parse_etime("07:45"), 465)
        self.assertEqual(ml.parse_etime("01:28:35"), 5315)
        self.assertEqual(ml.parse_etime("1-01:00:00"), 90000)

    def test_step_naming(self):
        self.assertEqual(ml.step_of("cargo-clippy", "cargo-clippy clippy"), "clippy")
        self.assertEqual(ml.step_of("cargo", "cargo +stable doc --no-deps"), "doc")
        self.assertEqual(ml.step_of("cargo", "cargo nextest run"), "nextest")
        self.assertEqual(ml.step_of("cargo", "cargo fmt --check"), "other")
        self.assertEqual(ml.step_of("cargo", ""), "other")

    def test_docker_vm_and_sessions(self):
        r = reading()
        self.assertEqual(r.docker_rss, 776384 * 1024)
        self.assertEqual(
            (r.sessions.live, r.sessions.building, r.sessions.free), (2, 2, 0)
        )


class BuildQueue(unittest.TestCase):
    def setUp(self):
        self.q = reading().queue

    def test_compiler_children_mean_building(self):
        doc = [b for b in self.q.building if b.pid == 10069]
        self.assertEqual(doc[0].step, "doc")
        self.assertEqual(doc[0].compilers["rustdoc"], 2)

    def test_a_running_test_binary_is_building(self):
        self.assertIn(30000, [b.pid for b in self.q.building])

    def test_a_wrapper_and_its_cargo_are_one_waiting_invocation(self):
        waiting = {b.pid: b for b in self.q.waiting}
        self.assertIn(16048, waiting)
        self.assertNotIn(16112, waiting)
        self.assertEqual(waiting[16048].step, "clippy")

    def test_a_fresh_cargo_waits_seconds(self):
        fresh = [b for b in self.q.waiting if b.pid == 61255][0]
        self.assertEqual((fresh.step, fresh.elapsed_s), ("check", 3))

    def test_longest_waiter(self):
        self.assertEqual((self.q.longest.pid, self.q.longest.elapsed_s), (74477, 465))

    def test_vanished_args_read_other(self):
        def fake(cmd):
            if "-p" in cmd:
                raise ml.NoReading("ps exited 1")
            return runner()(cmd)

        q = ml.read_machine(fake).queue
        # `cargo-clippy` names its step from its comm, args or none.
        self.assertEqual({b.step for b in q.waiting}, {"clippy", "other"})

    def test_a_stopped_cargo_and_a_watcher_are_not_waiting(self):
        pids = {b.pid for b in self.q.waiting + self.q.building}
        self.assertNotIn(40000, pids)
        self.assertNotIn(40100, pids)


class Verdict(unittest.TestCase):
    def test_a_long_wait_refuses_building_but_not_free(self):
        r = reading()
        k = knobs(r)
        self.assertEqual(ml.verdict(r, k, "building"), (0, ["wait"]))
        # Two live clients of 16: the rest of the total is free's.
        self.assertEqual(ml.verdict(r, k, "free"), (14, []))

    def test_anyone_waiting_admits_one_at_a_time(self):
        r = reading()
        self.assertEqual(
            ml.verdict(r, knobs(r, DS_FLEET_BUILD_WAIT_S=600), "building"), (1, [])
        )

    def test_an_empty_queue_admits_up_to_the_total(self):
        quiet = [
            line
            for line in PS.splitlines()
            if not line.endswith(("cargo", "cargo-clippy"))
        ]
        r = reading(ps="\n".join(quiet))
        self.assertEqual(ml.verdict(r, knobs(r, DS_FLEET_TOTAL=5), "building"), (3, []))

    def test_the_total_bounds_a_free_verb(self):
        r = reading()
        self.assertEqual(ml.verdict(r, knobs(r, DS_FLEET_TOTAL=3), "free"), (1, []))

    def test_the_total_refuses_both_classes(self):
        r = reading()
        k = knobs(r, DS_FLEET_TOTAL=2, DS_FLEET_BUILD_WAIT_S=600)
        for cls in ("building", "free"):
            with self.subTest(cls=cls):
                self.assertEqual(ml.verdict(r, k, cls), (0, ["total"]))

    def test_the_load_gate_refuses_only_above_its_limit(self):
        for load, reasons in (("16.00", []), ("16.01", ["load"])):
            r = reading(ctl=sysctl(load=load))
            with self.subTest(load=load):
                self.assertEqual(ml.verdict(r, knobs(r), "free")[1], reasons)

    def test_swap_is_reported_not_refused(self):
        r = reading(ctl=sysctl(swap="43.00M"))
        k = knobs(r, DS_FLEET_BUILD_WAIT_S=600)
        self.assertEqual(ml.verdict(r, k, "free"), (14, []))
        self.assertEqual(ml.verdict(r, k, "building"), (1, []))
        self.assertEqual(r.sysctl.swap_used, 43 * 1024**2)

    def test_each_machine_check_refuses_both_classes(self):
        cases = {
            "pressure": dict(ctl=sysctl(pressure=4)),
            "load": dict(ctl=sysctl(load="28.00")),
        }
        for reason, kw in cases.items():
            r = reading(**kw)
            k = knobs(r, DS_FLEET_BUILD_WAIT_S=600)
            for cls in ("building", "free"):
                with self.subTest(reason=reason, cls=cls):
                    self.assertEqual(ml.verdict(r, k, cls), (0, [reason]))

    def test_memory_reserve_refuses_both_classes(self):
        r = reading()
        k = knobs(r, DS_FLEET_RESERVE_GB=14, DS_FLEET_BUILD_WAIT_S=600)
        self.assertEqual(ml.verdict(r, k, "free"), (0, ["memory"]))
        self.assertEqual(ml.verdict(r, k, "building"), (0, ["memory"]))
        self.assertEqual(ml.headroom(r, k), 0)

    def test_reserve_defaults_to_an_eighth(self):
        r = reading()
        self.assertEqual(knobs(r).reserve, 6 * GIB)

    def test_a_bad_optional_knob_falls_back_with_a_note(self):
        r = reading()
        k = knobs(r, DS_FLEET_BUILD_WAIT_S="lots", DS_FLEET_RESERVE_GB="inf")
        self.assertEqual((k.build_wait_s, k.reserve), (120, 6 * GIB))
        self.assertEqual(len(k.notes), 2)
        self.assertIn("DS_FLEET_BUILD_WAIT_S", k.notes[0])

    def test_an_unset_or_bad_required_knob_is_no_reading_naming_it(self):
        r = reading()
        for var in FLEET:
            for raw in ("", "lots", "nan"):
                with self.subTest(var=var, raw=raw):
                    with self.assertRaises(ml.NoReading) as caught:
                        knobs(r, **{var: raw})
                    self.assertIn(var, str(caught.exception))

    def test_status(self):
        self.assertEqual(ml.status(3, 3), ml.GREEN)
        self.assertEqual(ml.status(1, 3), ml.AMBER)
        self.assertEqual(ml.status(0, 1), ml.RED)


class Sessions(unittest.TestCase):
    def test_only_records_of_live_claude_pids_count(self):
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp)
            (state / "eng-1.class").write_text("free 3326\n")
            (state / "sessions").mkdir()
            (state / "sessions" / "abc.class").write_text("building 94579\n")
            # A dead session's record, one with no pid and an undecodable one.
            (state / "eng-2.class").write_text("free 55555\n")
            (state / "eng-3.class").write_text("free\n")
            (state / "eng-4.class").write_bytes(b"\xff\xfe")
            s = ml.read_machine(runner(), state).sessions
        self.assertEqual((s.live, s.building, s.free, s.records), (2, 1, 1, 2))


class Main(unittest.TestCase):
    def call(self, *argv, fake=None, env=None):
        out, err = io.StringIO(), io.StringIO()
        with (
            tempfile.TemporaryDirectory() as tmp,
            redirect_stdout(out),
            redirect_stderr(err),
        ):
            code = ml.main(
                [*argv, "--state-dir", tmp],
                runner=fake or runner(),
                env={**FLEET, **(env or {})},
            )
            log = Path(tmp, ml.SAMPLE_LOG)
            sample = log.read_text() if log.exists() else None
        return code, out.getvalue(), err.getvalue(), sample

    def test_default_output_names_the_refusal(self):
        code, out, _, _ = self.call()
        self.assertEqual(code, ml.RED)
        self.assertTrue(out.startswith("machine-load: RED - 0 slots for building"))
        self.assertIn("longest 7m45s (pid 74477); limit 2m00s", out)
        self.assertIn(
            "35.0 of 48.0 GiB in use (app 21.0, wired 4.0, compressed 10.0)", out
        )
        self.assertIn("would free a slot: the longest wait falling under 2m00s", out)
        self.assertIn("clients   2 of 16 (builders 2, free 0)", out)
        self.assertIn("load      10.8 on 18 cores (limit 16)", out)
        self.assertIn("free: 14 slots", out)

    def test_an_unset_knob_exits_no_reading_naming_it(self):
        code, out, err, _ = self.call(env={"DS_FLEET_TOTAL": ""})
        self.assertEqual(code, ml.NO_READING)
        self.assertIn("DS_FLEET_TOTAL is unset", err)
        self.assertEqual(out, "")

    def test_amber_when_fewer_fit_than_asked(self):
        code, out, _, _ = self.call(
            "--sessions", "3", env={"DS_FLEET_BUILD_WAIT_S": "600"}
        )
        self.assertEqual(code, ml.AMBER)
        self.assertIn("AMBER - 1 slot for building (3 asked)", out)

    def test_a_machine_refusal_renders_for_both_classes(self):
        code, out, _, _ = self.call(
            "--class", "free", env={"DS_FLEET_RESERVE_GB": "14"}
        )
        self.assertEqual(code, ml.RED)
        self.assertIn("would free a slot: memory headroom; the longest wait", out)
        self.assertIn("free: refused on memory (headroom 0.0 GiB)", out)

    def test_the_total_and_load_refusals_render_their_numbers(self):
        code, out, _, _ = self.call(
            "--class",
            "free",
            env={"DS_FLEET_TOTAL": "2", "DS_FLEET_CPU_LOAD_MAX": "10"},
        )
        self.assertEqual(code, ml.RED)
        self.assertIn("free: refused on load (10.8, limit 10); clients (2 of 2)", out)
        self.assertIn(
            "would free a slot: load falling under 10; a session closing (total 2)",
            out,
        )

    def test_a_crash_is_no_reading_not_amber(self):
        def boom(cmd):
            raise RuntimeError("unexpected")

        code, _, err, _ = self.call(fake=boom)
        self.assertEqual(code, ml.NO_READING)
        self.assertIn("RuntimeError: unexpected", err)

    def test_a_bad_argument_is_no_reading_not_red(self):
        with self.assertRaises(SystemExit) as caught:
            self.call("--sessions", "0")
        self.assertEqual(caught.exception.code, ml.NO_READING)

    def test_free_class_green(self):
        code, _, _, _ = self.call("--class", "free", "--sessions", "4")
        self.assertEqual(code, ml.GREEN)

    def test_json_fields(self):
        code, out, _, _ = self.call("--json")
        data = json.loads(out)
        self.assertEqual((code, data["status"], data["exit"]), (ml.RED, "red", 2))
        self.assertEqual(data["verdict"]["building"], {"slots": 0, "reasons": ["wait"]})
        self.assertEqual(data["verdict"]["free"], {"slots": 14, "reasons": []})
        self.assertEqual(data["swap_used"], 0)
        self.assertEqual(data["builds"]["longest_wait_s"], 465)
        self.assertEqual(data["memory"]["in_use"], 35 * GIB)

    def test_sample_appends_one_line(self):
        _, _, _, sample = self.call("--sample")
        fields = sample.strip().split(" ")
        self.assertEqual(len(sample.splitlines()), 1)
        self.assertEqual(
            fields[1:],
            [
                "builders=2",
                "builds=2",
                "waiting=check:1,clippy:1,test:1",
                "longest_s=465",
                "in_use_gib=35.0",
            ],
        )

    def test_a_parse_failure_is_no_reading(self):
        code, out, err, _ = self.call(fake=runner(vm="garbage"))
        self.assertEqual(code, ml.NO_READING)
        self.assertIn("NO READING", err)
        self.assertEqual(out, "")

    def test_no_reading_json(self):
        code, out, _, _ = self.call("--json", fake=runner(ctl="1\n2\n"))
        self.assertEqual(code, ml.NO_READING)
        self.assertEqual(json.loads(out)["status"], "no-reading")


class LaunchRecord(unittest.TestCase):
    def record(self, *argv, fake=None, env=None):
        with tempfile.TemporaryDirectory() as tmp, redirect_stdout(io.StringIO()):
            path = Path(tmp, "sessions", "abc.load")
            for _ in range(2):
                code = ml.main(
                    [*argv, "--state-dir", tmp, "--record", str(path)],
                    runner=fake or runner(),
                    env={**FLEET, **(env or {})},
                )
            lines = path.read_text().splitlines()
        return code, [json.loads(line) for line in lines]

    def test_each_launch_appends_the_reading(self):
        _, lines = self.record("--json", "--class", "free", env={"DS_CARGO_LANES": "2"})
        self.assertEqual(len(lines), 2)
        rec = lines[0]
        self.assertRegex(rec["at"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
        self.assertEqual(
            {k: rec[k] for k in ("class", "status", "lanes", "building", "free")},
            {"class": "free", "status": "green", "lanes": 2, "building": 2, "free": 0},
        )
        self.assertEqual(
            (rec["queue_building"], rec["queue_waiting"], rec["longest_wait_s"]),
            (2, 3, 465),
        )
        self.assertEqual((rec["in_use"], rec["compressed"]), (35 * GIB, 10 * GIB))
        self.assertEqual((rec["load1"], rec["swap_used"], rec["live"]), (10.8, 0, 2))
        self.assertEqual(rec["verdict"], {"slots": 14, "reasons": []})

    def test_the_raw_swap_figure_rides_the_record_and_admits(self):
        _, lines = self.record(
            "--class", "free", fake=runner(ctl=sysctl(swap="43.00M"))
        )
        self.assertEqual(lines[0]["swap_used"], 43 * 1024**2)
        self.assertEqual(lines[0]["status"], "green")

    def test_the_verdict_is_the_launching_class(self):
        _, lines = self.record()
        self.assertEqual(lines[0]["verdict"], {"slots": 0, "reasons": ["wait"]})
        # Required from the runtime config: unset records null, never a guess.
        self.assertIsNone(lines[0]["lanes"])

    def test_the_launch_call_samples_and_records_together(self):
        with tempfile.TemporaryDirectory() as tmp, redirect_stdout(io.StringIO()):
            path = Path(tmp, "abc.load")
            code = ml.main(
                ["--json", "--sample", "--state-dir", tmp, "--record", str(path)],
                runner=runner(),
                env=FLEET,
            )
            self.assertEqual(len(Path(tmp, ml.SAMPLE_LOG).read_text().splitlines()), 1)
            self.assertEqual(len(path.read_text().splitlines()), 1)
        self.assertEqual(code, ml.RED)

    def test_an_unwritable_record_is_a_warning_not_no_reading(self):
        with (
            tempfile.TemporaryDirectory() as tmp,
            redirect_stdout(io.StringIO()) as out,
            redirect_stderr(io.StringIO()) as err,
        ):
            blocker = Path(tmp, "file")
            blocker.write_text("")
            code = ml.main(
                ["--json", "--state-dir", tmp, "--record", str(blocker / "x.load")],
                runner=runner(),
                env=FLEET,
            )
        self.assertEqual(code, ml.RED)
        self.assertEqual(json.loads(out.getvalue())["status"], "red")
        self.assertIn("could not append", err.getvalue())

    def test_no_reading_still_leaves_a_line(self):
        code, lines = self.record(fake=runner(vm="garbage"))
        self.assertEqual(code, ml.NO_READING)
        self.assertEqual(lines[0]["status"], "no-reading")
        self.assertIn("error", lines[0])


if __name__ == "__main__":
    unittest.main()
