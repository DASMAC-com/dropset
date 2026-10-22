#!/usr/bin/env python3
# cspell:word getaffinity
"""`_ds_cargo_pool` — the shared build directory and job bound the init exports.

Each case sources the real init in a fresh zsh, changes directory the way a
shell does, and reads the two variables back, so what is under test is what an
operator's shell actually carries. Two cases are load-bearing: the off switch
(`DS_CARGO_POOL=0` must leave both unset, or the documented switch does not
restore today's behavior) and the scope (outside a dropset checkout both must be
unset, because in another Anchor repo a pooled target dir re-keys its program).
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

INIT = Path(__file__).resolve().parents[1] / "shell" / "init.zsh"

#: The checkout the init treats as the repo: sourcing this copy makes its own
#: root `_DS_REPO`, so "inside" is any directory under it.
REPO = INIT.parents[2]

ZSH = shutil.which("zsh")

# Same guard and reason as the sibling shell suites: the helpers are zsh and the
# Linux CI runner ships none unless the workflow installs it.
_NEEDS_ZSH = "the session helpers are zsh; no zsh on this machine"

#: Cleared from the inherited environment so a case tests the helper rather than
#: the operator's own shell, which (once this lands) exports all of them.
_SUITE_OWNED_ENV = (
    "CARGO_BUILD_JOBS",
    "CARGO_HOME",
    "CARGO_TARGET_DIR",
    "CLIPPY_CONF_DIR",
    "DS_CARGO_POOL",
    "RUSTC_WORKSPACE_WRAPPER",
    "_DS_REPO",
)

#: The per-checkout key, relative to a checkout root.
WRAPPER = Path(".claude") / "shell" / "rustc-workspace-wrapper"

_KEYS = ("RUSTC_WORKSPACE_WRAPPER", "CLIPPY_CONF_DIR")

_UNSET = "<unset>"


def _cores() -> int:
    """The count `nproc` reports on Linux (it honors affinity), else the CPUs."""
    if hasattr(os, "sched_getaffinity"):
        return len(os.sched_getaffinity(0))
    return os.cpu_count() or 0


@unittest.skipUnless(ZSH, _NEEDS_ZSH)
class CargoPoolHarness(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.outside = self._tmp.name

    def _pool(
        self,
        *dirs,
        env=None,
        resource=False,
        pin="",
        init=INIT,
        names=("CARGO_TARGET_DIR", "CARGO_BUILD_JOBS"),
    ):
        """Source `init` in `dirs[0]`, `cd` through the rest, and return the
        `names` variables as the last directory sees them.
        `resource` sources it a second time first, as `_ds_reload_init` does;
        `pin` runs before the hops, the way an operator pins a value by hand."""
        start, *rest = dirs
        source = 'source "%s" 2>/dev/null; ' % init
        hops = "".join('cd "%s"; ' % d for d in rest)
        script = 'cd "%s"; %s%s%s%s' % (
            start,
            source,
            source if resource else "",
            pin,
            hops,
        )
        script += "; ".join('print -r -- "${%s-%s}"' % (n, _UNSET) for n in names)
        child_env = {**os.environ}
        for name in _SUITE_OWNED_ENV:
            child_env.pop(name, None)
        child_env.update(env or {})
        result = subprocess.run(
            [ZSH, "-c", script],
            capture_output=True,
            text=True,
            check=True,
            env=child_env,
        )
        return tuple(result.stdout.splitlines())


class TheSwitchOffLeavesBothUnset(CargoPoolHarness):
    def test_switch_off_exports_neither_variable(self):
        self.assertEqual(self._pool(REPO, env={"DS_CARGO_POOL": "0"}), (_UNSET, _UNSET))


class OnlyADropsetCheckoutPools(CargoPoolHarness):
    def test_another_repo_gets_neither_variable(self):
        self.assertEqual(self._pool(self.outside), (_UNSET, _UNSET))

    def test_a_subdirectory_of_the_checkout_pools(self):
        target, _ = self._pool(REPO / "tui", env={"CARGO_HOME": "/opt/cargo"})
        self.assertEqual(target, "/opt/cargo/target/dropset")

    def test_cd_into_the_checkout_pools(self):
        target, _ = self._pool(self.outside, REPO, env={"CARGO_HOME": "/opt/cargo"})
        self.assertEqual(target, "/opt/cargo/target/dropset")

    def test_leaving_the_checkout_unsets_both(self):
        self.assertEqual(self._pool(REPO, self.outside), (_UNSET, _UNSET))

    def test_a_reload_still_unsets_on_leaving(self):
        self.assertEqual(
            self._pool(REPO, self.outside, resource=True), (_UNSET, _UNSET)
        )


class TheDefaultPools(CargoPoolHarness):
    def test_the_directory_sits_under_cargo_home(self):
        target, _ = self._pool(REPO, env={"CARGO_HOME": "/opt/cargo"})
        self.assertEqual(target, "/opt/cargo/target/dropset")

    def test_an_unset_cargo_home_means_the_default_one(self):
        target, _ = self._pool(REPO, env={"HOME": "/home/user"})
        self.assertEqual(target, "/home/user/.cargo/target/dropset")

    def test_the_job_bound_is_the_cores_minus_two(self):
        cores = _cores()
        _, jobs = self._pool(REPO)
        self.assertEqual(jobs, str(cores - 2) if cores > 2 else _UNSET)


class AnExportedValueWins(CargoPoolHarness):
    PINNED = {"CARGO_TARGET_DIR": "/scratch/t", "CARGO_BUILD_JOBS": "3"}

    def test_an_operator_pinned_value_is_not_overwritten(self):
        self.assertEqual(self._pool(REPO, env=self.PINNED), ("/scratch/t", "3"))

    def test_a_pinned_value_survives_leaving_the_checkout(self):
        self.assertEqual(
            self._pool(REPO, self.outside, env=self.PINNED), ("/scratch/t", "3")
        )

    def test_a_pin_made_after_the_export_survives_leaving(self):
        pin = "export CARGO_TARGET_DIR=/scratch/t; "
        target, jobs = self._pool(REPO, self.outside, pin=pin)
        self.assertEqual((target, jobs), ("/scratch/t", _UNSET))


class EachCheckoutGetsItsOwnKeys(CargoPoolHarness):
    """The per-checkout keys, against a fake base repo with two worktrees: one
    carrying the wrapper, one whose branch predates it. `:A` resolves the temp
    directory through any symlink, so expectations use the resolved path."""

    def setUp(self):
        super().setUp()
        self.base = Path(self.outside).resolve() / "repo"
        self.init = self.base / ".claude" / "shell" / "init.zsh"
        self.init.parent.mkdir(parents=True)
        shutil.copy(INIT, self.init)
        self.worktree = self.base / ".claude" / "worktrees" / "7"
        self.stale = self.base / ".claude" / "worktrees" / "6"
        for checkout in (self.base, self.worktree):
            wrapper = checkout / WRAPPER
            wrapper.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(REPO / WRAPPER, wrapper)
        (self.worktree / "tui").mkdir()
        self.stale.mkdir(parents=True)

    def _keys(self, *dirs, **kwargs):
        return self._pool(*dirs, init=self.init, names=_KEYS, **kwargs)

    def test_the_base_repo_keys_on_itself(self):
        self.assertEqual(
            self._keys(self.base), (str(self.base / WRAPPER), str(self.base))
        )

    def test_a_worktree_subdirectory_keys_on_the_worktree(self):
        self.assertEqual(
            self._keys(self.worktree / "tui"),
            (str(self.worktree / WRAPPER), str(self.worktree)),
        )

    def test_the_keys_follow_a_cd_between_checkouts(self):
        self.assertEqual(
            self._keys(self.base, self.worktree),
            (str(self.worktree / WRAPPER), str(self.worktree)),
        )

    def test_a_branch_without_the_wrapper_gets_no_keys(self):
        self.assertEqual(self._keys(self.worktree, self.stale), (_UNSET, _UNSET))

    def test_leaving_the_checkout_unsets_the_keys(self):
        self.assertEqual(self._keys(self.worktree, self.outside), (_UNSET, _UNSET))

    def test_the_switch_off_exports_no_keys(self):
        self.assertEqual(
            self._keys(self.worktree, env={"DS_CARGO_POOL": "0"}), (_UNSET, _UNSET)
        )

    def test_a_pinned_key_survives_a_cd_between_checkouts(self):
        pinned = {"CLIPPY_CONF_DIR": "/scratch/c"}
        _, conf = self._keys(self.base, self.worktree, env=pinned)
        self.assertEqual(conf, "/scratch/c")

    def test_the_wrapper_passes_its_command_through(self):
        out = subprocess.run(
            [str(REPO / WRAPPER), "printf", "%s|", "a b", "c"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        self.assertEqual(out, "a b|c|")


if __name__ == "__main__":
    unittest.main()
