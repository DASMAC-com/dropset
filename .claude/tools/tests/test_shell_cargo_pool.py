"""Tests for ``_ds_cargo_pool`` in ``.claude/shell/init.zsh``: the target
directory is the checkout's lane, a worktree on its issue number modulo
``DS_CARGO_LANES`` and the base repo on lane 0, and the per-checkout keys are
the checkout's own — evaluated for a directory the launcher names, not only for
``$PWD``.

The function is sourced from a copy of the file placed in a throwaway tree (the
file derives its repo root from its own location). Skipped where ``zsh`` is
absent.
"""

from __future__ import annotations

import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

INIT = Path(__file__).resolve().parents[2] / "shell" / "init.zsh"
WRAPPER = Path(".claude") / "shell" / "rustc-workspace-wrapper"


@unittest.skipUnless(shutil.which("zsh"), "needs zsh")
class CargoPoolTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name).resolve()
        self.repo = self.root / "repo"
        self.pool_dir = self.root / "cargo" / "target" / "dropset"
        for checkout in (self.repo, *map(self.worktree, ("2260", "2261"))):
            self.add_wrapper(checkout)
        shutil.copy(INIT, self.repo / ".claude" / "shell" / "init.zsh")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def worktree(self, tag: str) -> Path:
        return self.repo / ".claude" / "worktrees" / tag

    def add_wrapper(self, checkout: Path) -> None:
        wrapper = checkout / WRAPPER
        wrapper.parent.mkdir(parents=True, exist_ok=True)
        wrapper.write_text('#!/bin/sh\nexec "$@"\n')
        wrapper.chmod(0o755)

    def pool(self, cwd: Path, *args: str, **env: str) -> tuple[str, str]:
        """(CARGO_TARGET_DIR, RUSTC_WORKSPACE_WRAPPER) after the hook runs."""
        script = (
            'source "$1" && shift && _ds_cargo_pool "$@" && '
            'print -r -- "${CARGO_TARGET_DIR-}|${RUSTC_WORKSPACE_WRAPPER-}"'
        )
        init = str(self.repo / ".claude" / "shell" / "init.zsh")
        result = subprocess.run(
            ["zsh", "-fc", script, "zsh", init, *args],
            cwd=cwd,
            capture_output=True,
            text=True,
            env={
                "PATH": "/usr/bin:/bin:/usr/sbin",
                "HOME": str(self.root),
                "CARGO_HOME": str(self.root / "cargo"),
                **env,
            },
            check=True,
        )
        target, _, wrapper = result.stdout.strip().partition("|")
        return target, wrapper

    def lane(self, n: int) -> str:
        return f"{self.pool_dir}-{n}" if n else str(self.pool_dir)

    def test_the_base_repo_is_lane_zero_the_original_directory(self) -> None:
        self.assertEqual(self.pool(self.repo), (self.lane(0), str(self.repo / WRAPPER)))

    def test_a_worktree_takes_its_number_modulo_the_default_two_lanes(self) -> None:
        for tag, lane in (("2260", 0), ("2261", 1)):
            wt = self.worktree(tag)
            self.assertEqual(self.pool(wt), (self.lane(lane), str(wt / WRAPPER)))

    def test_one_lane_is_the_single_pool(self) -> None:
        target, _ = self.pool(self.worktree("2261"), DS_CARGO_LANES="1")
        self.assertEqual(target, self.lane(0))

    def test_a_malformed_lane_count_falls_back_to_the_single_pool(self) -> None:
        for bad in ("0", "two", "-3"):
            target, _ = self.pool(self.worktree("2261"), DS_CARGO_LANES=bad)
            self.assertEqual(target, self.lane(0), bad)

    def test_a_prefixed_tag_keys_on_its_number_and_a_worded_one_on_lane_zero(
        self,
    ) -> None:
        for tag, lane in (("eng-2263", 1), ("scratch", 0)):
            wt = self.worktree(tag)
            self.add_wrapper(wt)
            target, _ = self.pool(wt, DS_CARGO_LANES="3")
            self.assertEqual(target, self.lane(lane), tag)

    def test_the_launcher_evaluates_for_the_worktree_it_names(self) -> None:
        wt = self.worktree("2261")
        self.assertEqual(
            self.pool(self.repo, str(wt)), (self.lane(1), str(wt / WRAPPER))
        )

    def test_a_new_session_starts_in_its_worktree_s_pool_and_the_tab_reverts(
        self,
    ) -> None:
        """`_ds_session` launches from the base checkout; the stub `claude`
        prints the pool it inherited, then the tab's own pool is printed."""
        probe = 'print -r -- "${CARGO_TARGET_DIR-}|${RUSTC_WORKSPACE_WRAPPER-}"'
        stubs = " ".join(
            f"{fn}() {{ :; }};"
            for fn in (
                "_ds_secrets",
                "_ds_session_mark",
                "_ds_worktree_ensure",
                "_ds_exit_estimate",
                "_ds_op_signout",
            )
        )
        script = (
            f'source "$1" && {stubs} _ds_base() {{ cd "$_DS_REPO"; }}; '
            f"claude() {{ {probe}; }}; "
            f"_ds_session sid name '' model 2261 && {probe}"
        )
        init = str(self.repo / ".claude" / "shell" / "init.zsh")
        lines = subprocess.run(
            ["zsh", "-fc", script, "zsh", init],
            cwd=self.repo,
            capture_output=True,
            text=True,
            env={
                "PATH": "/usr/bin:/bin:/usr/sbin",
                "HOME": str(self.root),
                "CARGO_HOME": str(self.root / "cargo"),
            },
            check=True,
        ).stdout.split()
        wt = self.worktree("2261")
        self.assertEqual(
            lines,
            [
                f"{self.lane(1)}|{wt / WRAPPER}",
                f"{self.lane(0)}|{self.repo / WRAPPER}",
            ],
        )

    def test_an_inherited_lane_is_recomputed_and_a_hand_pin_survives(self) -> None:
        wt = self.worktree("2261")
        target, _ = self.pool(wt, CARGO_TARGET_DIR=self.lane(0))
        self.assertEqual(target, self.lane(1))
        pin = str(self.root / "elsewhere")
        target, _ = self.pool(wt, CARGO_TARGET_DIR=pin)
        self.assertEqual(target, pin)

    def test_leaving_the_repo_unsets_an_inherited_lane_but_not_a_pin(self) -> None:
        self.assertEqual(self.pool(self.root, CARGO_TARGET_DIR=self.lane(1))[0], "")
        pin = str(self.root / "elsewhere")
        self.assertEqual(self.pool(self.root, CARGO_TARGET_DIR=pin)[0], pin)


if __name__ == "__main__":
    unittest.main()
