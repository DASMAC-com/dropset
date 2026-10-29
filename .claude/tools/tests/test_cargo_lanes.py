"""Stdlib ``unittest`` tests for the cargo-lanes report and wipe.

Each case builds a throwaway target root and feeds ``ps`` and ``du`` output
through a fake runner, so the lane naming, the per-entry build attribution,
the fail-closed probe and the tombstone-then-remove wipe are asserted without
touching this machine's cargo home. Run via ``make tools-tests``.
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import cargo_lanes as cl

# pid ppid rss etime stat comm — a `cargo` root with a rustc child, a waiting
# `cargo-build-sbf` root, and the sbf root's own `cargo` child (not a root).
PS_TABLE = """\
  100     1  9000   01:00 S    cargo
  101   100  9000   00:30 R    rustc
  200     1  9000   00:10 S    cargo-build-sbf
  201   200  9000   00:05 S    cargo
"""
ONE_ROOT = PS_TABLE.split("  200")[0]
NO_CARGO = "    1     0  9000   01:00 S    launchd\n"


class FakeRun:
    """Answers `ps` and `du` from canned text; records every call."""

    def __init__(self, table=PS_TABLE, commands=None, sizes=None, table_rc=0):
        self.table, self.table_rc = table, table_rc
        self.commands = commands or {}
        self.sizes = sizes or {}
        self.calls: list[list[str]] = []

    def __call__(self, cmd):
        self.calls.append(cmd)
        if cmd[0] == "du":
            kib = self.sizes.get(Path(cmd[-1]).name)
            return (0, f"{kib}\t{cmd[-1]}\n") if kib is not None else (1, "")
        if "-E" in cmd:
            pids = [int(p) for p in cmd[-1].split(",")]
            lines = [f"{p} {self.commands[p]}" for p in pids if p in self.commands]
            return (0 if len(lines) == len(pids) else 1, "\n".join(lines) + "\n")
        return self.table_rc, self.table


class LaneNaming(unittest.TestCase):
    def test_the_count_mirrors_the_hook(self):
        self.assertEqual(cl.lane_count({}), 2)
        self.assertEqual(cl.lane_count({"DS_CARGO_LANES": "3"}), 3)
        # The hook falls back to one lane on anything but a whole number >= 1.
        self.assertEqual(cl.lane_count({"DS_CARGO_LANES": "0"}), 1)
        self.assertEqual(cl.lane_count({"DS_CARGO_LANES": "two"}), 1)

    def test_lane_zero_is_the_bare_name(self):
        self.assertEqual(cl.lane_names(3), ["dropset", "dropset-1", "dropset-2"])

    def test_the_root_follows_cargo_home(self):
        self.assertEqual(
            cl.target_root({"CARGO_HOME": "/c", "HOME": "/h"}), Path("/c/target")
        )
        self.assertEqual(cl.target_root({"HOME": "/h"}), Path("/h/.cargo/target"))


class Attribution(unittest.TestCase):
    root = Path("/c/target")

    def test_an_argument_beats_the_environment(self):
        line = "cargo build --target-dir /x CARGO_TARGET_DIR=/c/target/dropset"
        self.assertEqual(cl.target_of(line), "/x")
        self.assertEqual(
            cl.target_of("cargo test CARGO_TARGET_DIR=/c/target/dropset-1 HOME=/h"),
            "/c/target/dropset-1",
        )
        self.assertIsNone(cl.target_of("cargo build HOME=/h"))

    def test_an_entry_is_the_first_component_under_the_root(self):
        self.assertEqual(cl.entry_of("/c/target/dropset-1/sbf", self.root), "dropset-1")
        self.assertIsNone(cl.entry_of("/elsewhere/target", self.root))

    def test_each_root_is_attributed_to_its_lane(self):
        run = FakeRun(
            commands={
                100: "cargo test CARGO_TARGET_DIR=/c/target/dropset",
                200: "cargo-build-sbf CARGO_TARGET_DIR=/c/target/dropset-1",
            }
        )
        busy = cl.probe(run, self.root, alive=lambda _pid: True)
        self.assertEqual(busy, {"dropset": [100], "dropset-1": [200]})
        # The sbf root's own cargo child is not a root, so it is never asked.
        self.assertNotIn("201", run.calls[1][-1].split(","))

    def test_a_root_that_exited_between_reads_holds_nothing(self):
        run = FakeRun(commands={100: "cargo test CARGO_TARGET_DIR=/c/target/dropset"})
        busy = cl.probe(run, self.root, alive=lambda pid: pid != 200)
        self.assertEqual(busy, {"dropset": [100]})

    def test_a_live_unreadable_root_fails_closed(self):
        run = FakeRun(commands={100: "cargo test CARGO_TARGET_DIR=/c/target/dropset"})
        with self.assertRaises(cl.ProbeFailed):
            cl.probe(run, self.root, alive=lambda _pid: True)

    def test_an_unreadable_table_fails_closed(self):
        with self.assertRaises(cl.ProbeFailed):
            cl.probe(FakeRun(table_rc=1), self.root, alive=lambda _pid: True)


class Wipe(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.home = Path(tmp.name)
        self.root = self.home / "target"
        for name in ("dropset", "dropset-1", "a3", "dropset-7"):
            (self.root / name / "debug").mkdir(parents=True)
        self.env = {"CARGO_HOME": str(self.home), "DS_CARGO_LANES": "3"}
        self.sizes = {
            "dropset": 2 * 1024**2,
            "dropset-1": 1024**2,
            "a3": 4,
            "dropset-7": 8,
        }

    def run_report(self, apply, commands=None, table=PS_TABLE):
        run = FakeRun(table=table, commands=commands or {}, sizes=self.sizes)
        return cl.report(self.env, apply, run=run, alive=lambda _pid: False)

    def by_name(self, result):
        return {e["name"]: e for e in result["entries"]}

    def test_a_dry_run_touches_nothing(self):
        result = self.run_report(apply=False)
        self.assertTrue(result["dry_run"])
        self.assertEqual(
            sorted(p.name for p in self.root.iterdir()),
            ["a3", "dropset", "dropset-1", "dropset-7"],
        )
        entries = self.by_name(result)
        self.assertEqual(entries["dropset"]["action"], "would remove")
        # A lane the count names but no build created yet.
        self.assertEqual(entries["dropset-2"]["action"], "absent")
        # A lane beyond the count is a non-lane entry.
        self.assertFalse(entries["dropset-7"]["lane"])

    def test_the_wipe_skips_only_the_busy_lane(self):
        commands = {100: "cargo test CARGO_TARGET_DIR=" + str(self.root / "dropset-1")}
        result = self.run_report(apply=True, commands=commands, table=ONE_ROOT)
        self.assertEqual(sorted(p.name for p in self.root.iterdir()), ["dropset-1"])
        entries = self.by_name(result)
        self.assertEqual(
            entries["dropset-1"]["action"], "skipped: cargo build running (pid 100)"
        )
        self.assertEqual(entries["dropset"]["action"], "removed")
        self.assertEqual(result["freed"], (2 * 1024**2 + 4 + 8) * 1024)

    def test_a_failed_probe_holds_every_entry(self):
        run = FakeRun(table_rc=1, sizes=self.sizes)
        result = cl.report(self.env, True, run=run, alive=lambda _pid: False)
        self.assertTrue(result["probe"].startswith("failed"))
        self.assertEqual(len(list(self.root.iterdir())), 4)
        self.assertIn("build probe failed", cl.summary(result))

    def test_a_crashed_removal_leaves_a_tombstone_the_next_run_takes(self):
        def crash(path):
            raise OSError(5, "Input/output error")

        run = FakeRun(table=NO_CARGO, sizes=self.sizes)
        result = cl.report(
            self.env, True, run=run, alive=lambda _pid: False, remove=crash
        )
        self.assertEqual(
            self.by_name(result)["a3"]["action"],
            "failed mid-removal: Input/output error",
        )
        self.assertIn(".wipe-a3", [p.name for p in self.root.iterdir()])
        again = cl.report(
            self.env, True, run=FakeRun(table=NO_CARGO), alive=lambda _pid: False
        )
        self.assertEqual(self.by_name(again)[".wipe-a3"]["action"], "removed")
        self.assertEqual(list(self.root.iterdir()), [])

    def test_the_rows_name_each_lane_and_sweep_the_rest_into_one(self):
        rows = cl.rows(self.run_report(apply=False))
        self.assertEqual(
            rows,
            [
                "dropset 2.0 GiB — would remove",
                "dropset-1 1.0 GiB — would remove",
                "dropset-2 absent",
                "2 non-lane entries 0.0 MiB — would remove (largest: dropset-7, a3)",
            ],
        )


if __name__ == "__main__":
    unittest.main()
