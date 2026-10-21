#!/usr/bin/env python3
"""`_ds_cargo_pool` — the shared build directory and job bound the init exports.

Each case sources the real init in a fresh zsh and reads the two variables
back, so what is under test is what an operator's shell actually inherits. The
load-bearing case is the off switch: `DS_CARGO_POOL=0` must leave both unset, or
the switch the convention documents does not restore today's behavior.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import unittest
from pathlib import Path

INIT = Path(__file__).resolve().parents[1] / "shell" / "init.zsh"

# Same guard and reason as the sibling shell suites: the helpers are zsh and the
# Linux CI runner ships none unless the workflow installs it.
_NEEDS_ZSH = "the session helpers are zsh; no zsh on this machine"

#: Cleared from the inherited environment so a case tests the helper rather than
#: the operator's own shell, which (once this lands) exports all of them.
_SUITE_OWNED_ENV = (
    "CARGO_BUILD_JOBS",
    "CARGO_HOME",
    "CARGO_TARGET_DIR",
    "DS_CARGO_POOL",
    "_DS_REPO",
)

_UNSET = "<unset>"


@unittest.skipUnless(shutil.which("zsh"), _NEEDS_ZSH)
class CargoPoolHarness(unittest.TestCase):
    def _pool(self, env=None):
        """Source the init and return (CARGO_TARGET_DIR, CARGO_BUILD_JOBS)."""
        script = (
            'source "%s" 2>/dev/null; '
            'print -r -- "${CARGO_TARGET_DIR-%s}"; '
            'print -r -- "${CARGO_BUILD_JOBS-%s}"' % (INIT, _UNSET, _UNSET)
        )
        child_env = {**os.environ}
        for name in _SUITE_OWNED_ENV:
            child_env.pop(name, None)
        child_env.update(env or {})
        result = subprocess.run(
            [shutil.which("zsh"), "-c", script],
            capture_output=True,
            text=True,
            check=True,
            env=child_env,
        )
        target, jobs = result.stdout.splitlines()
        return target, jobs


class TheSwitchOffLeavesBothUnset(CargoPoolHarness):
    def test_switch_off_exports_neither_variable(self):
        self.assertEqual(self._pool({"DS_CARGO_POOL": "0"}), (_UNSET, _UNSET))


class TheDefaultPools(CargoPoolHarness):
    def test_the_directory_sits_under_cargo_home(self):
        target, _ = self._pool({"CARGO_HOME": "/opt/cargo"})
        self.assertEqual(target, "/opt/cargo/target/dropset")

    def test_an_unset_cargo_home_means_the_default_one(self):
        target, _ = self._pool({"HOME": "/home/user"})
        self.assertEqual(target, "/home/user/.cargo/target/dropset")

    def test_the_job_bound_is_the_cores_minus_two(self):
        cores = os.cpu_count() or 0
        _, jobs = self._pool()
        if cores > 2:
            self.assertEqual(jobs, str(cores - 2))
        else:
            self.assertEqual(jobs, _UNSET)


class AnExportedValueWins(CargoPoolHarness):
    def test_an_operator_pinned_value_is_not_overwritten(self):
        pinned = {"CARGO_TARGET_DIR": "/scratch/t", "CARGO_BUILD_JOBS": "3"}
        self.assertEqual(self._pool(pinned), ("/scratch/t", "3"))


if __name__ == "__main__":
    unittest.main()
