#!/usr/bin/env python3
# cspell:word getaffinity
"""`_ds_cargo_pool` — the build-directory lane and job bound the init exports.

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
    "DS_CARGO_LANES",
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

    def _inherited(self, checkout):
        """The keys a child shell inherits from one keyed on `checkout`: the
        exports arrive, the ownership map does not."""
        return {
            "RUSTC_WORKSPACE_WRAPPER": str(checkout / WRAPPER),
            "CLIPPY_CONF_DIR": str(checkout),
        }

    def test_an_inherited_key_still_follows_a_cd(self):
        self.assertEqual(
            self._keys(self.worktree, self.base, env=self._inherited(self.worktree)),
            (str(self.base / WRAPPER), str(self.base)),
        )

    def test_an_inherited_key_is_unset_on_leaving(self):
        self.assertEqual(
            self._keys(self.outside, env=self._inherited(self.worktree)),
            (_UNSET, _UNSET),
        )

    def test_an_inherited_key_is_dropped_on_a_stale_branch(self):
        self.assertEqual(
            self._keys(self.stale, env=self._inherited(self.worktree)),
            (_UNSET, _UNSET),
        )

    def test_a_pin_of_another_shape_is_not_mistaken_for_a_key(self):
        pinned = {
            "RUSTC_WORKSPACE_WRAPPER": str(self.worktree / "other-wrapper"),
            "CLIPPY_CONF_DIR": str(self.worktree / "tui"),
        }
        self.assertEqual(
            self._keys(self.base, self.outside, env=pinned),
            (pinned["RUSTC_WORKSPACE_WRAPPER"], pinned["CLIPPY_CONF_DIR"]),
        )

    def test_the_wrapper_passes_its_command_through(self):
        out = subprocess.run(
            [str(REPO / WRAPPER), "printf", "%s|", "a b", "c"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        self.assertEqual(out, "a b|c|")


class EachWorktreeGetsItsLane(CargoPoolHarness):
    """The lanes, against a fake base repo: a worktree builds in lane <its
    issue number> modulo `DS_CARGO_LANES` (default 2), the base repo and a
    worded tag in lane 0, which is the original `dropset` directory."""

    POOL = "/opt/cargo/target/dropset"

    def setUp(self):
        super().setUp()
        self.base = Path(self.outside).resolve() / "repo"
        self.init = self.base / ".claude" / "shell" / "init.zsh"
        self.init.parent.mkdir(parents=True)
        shutil.copy(INIT, self.init)
        for checkout in (self.base, *map(self._wt, ("2260", "2261", "eng-2263"))):
            wrapper = checkout / WRAPPER
            wrapper.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(REPO / WRAPPER, wrapper)
        self._wt("scratch").mkdir()

    def _wt(self, tag):
        return self.base / ".claude" / "worktrees" / tag

    def _lane(self, n):
        return f"{self.POOL}-{n}" if n else self.POOL

    def _target(self, *dirs, lanes=None, **kwargs):
        env = {"CARGO_HOME": "/opt/cargo", **kwargs.pop("env", {})}
        if lanes is not None:
            env["DS_CARGO_LANES"] = lanes
        (target,) = self._pool(
            *dirs, init=self.init, names=("CARGO_TARGET_DIR",), env=env, **kwargs
        )
        return target

    def test_the_base_repo_is_lane_zero(self):
        self.assertEqual(self._target(self.base), self._lane(0))

    def test_a_worktree_takes_its_number_modulo_two_by_default(self):
        self.assertEqual(self._target(self._wt("2260")), self._lane(0))
        self.assertEqual(self._target(self._wt("2261")), self._lane(1))

    def test_one_lane_is_the_single_pool(self):
        self.assertEqual(self._target(self._wt("2261"), lanes="1"), self._lane(0))

    def test_a_malformed_lane_count_is_the_single_pool(self):
        for bad in ("0", "two", "-3"):
            with self.subTest(bad=bad):
                self.assertEqual(
                    self._target(self._wt("2261"), lanes=bad), self._lane(0)
                )

    def test_a_prefixed_tag_keys_on_its_number_a_worded_one_on_lane_zero(self):
        self.assertEqual(self._target(self._wt("eng-2263"), lanes="3"), self._lane(1))
        self.assertEqual(self._target(self._wt("scratch"), lanes="3"), self._lane(0))

    def test_the_lane_follows_a_cd_between_checkouts(self):
        self.assertEqual(self._target(self._wt("2261"), self.base), self._lane(0))

    def test_an_inherited_lane_is_recomputed(self):
        env = {"CARGO_TARGET_DIR": self._lane(0)}
        self.assertEqual(self._target(self._wt("2261"), env=env), self._lane(1))

    def test_an_inherited_lane_is_unset_on_leaving(self):
        env = {"CARGO_TARGET_DIR": self._lane(1)}
        self.assertEqual(self._target(self.outside, env=env), _UNSET)

    def test_a_named_directory_is_evaluated_instead_of_the_cwd(self):
        """The launcher's form: from the base checkout, for the worktree it
        is about to start."""
        wt = self._wt("2261")
        pin = '_ds_cargo_pool "%s"; ' % wt
        self.assertEqual(
            self._pool(
                self.base,
                init=self.init,
                pin=pin,
                env={"CARGO_HOME": "/opt/cargo"},
                names=("CARGO_TARGET_DIR", "RUSTC_WORKSPACE_WRAPPER"),
            ),
            (self._lane(1), str(wt / WRAPPER)),
        )

    def test_a_new_session_inherits_its_worktree_s_pool_and_the_tab_reverts(self):
        """`_ds_session` launches from the base checkout; a stub `claude`
        prints the pool it inherited, then the tab's own pool is read back."""
        probe = (
            'print -r -- "$CARGO_TARGET_DIR"; print -r -- "$RUSTC_WORKSPACE_WRAPPER"'
        )
        stubs = "".join(
            "%s() { :; }; " % fn
            for fn in (
                "_ds_secrets",
                "_ds_session_mark",
                "_ds_worktree_ensure",
                "_ds_exit_estimate",
                "_ds_op_signout",
            )
        )
        pin = (
            stubs
            + '_ds_base() { cd "$_DS_REPO"; }; '
            + "claude() { %s; }; " % probe
            + "_ds_session sid name '' model 2261; "
        )
        wt = self._wt("2261")
        self.assertEqual(
            self._pool(
                self.base,
                init=self.init,
                pin=pin,
                env={"CARGO_HOME": "/opt/cargo", "HOME": self.outside},
                names=("CARGO_TARGET_DIR", "RUSTC_WORKSPACE_WRAPPER"),
            ),
            (
                self._lane(1),
                str(wt / WRAPPER),
                self._lane(0),
                str(self.base / WRAPPER),
            ),
        )


if __name__ == "__main__":
    unittest.main()
