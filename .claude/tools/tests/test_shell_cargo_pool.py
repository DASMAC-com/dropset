"""Tests for the cargo pool's per-checkout keys in ``.claude/shell/init.zsh``.

Two halves. The launcher half sources a copy of the file placed in a throwaway
base checkout (the file derives its repo root from its own location) with a
stub ``claude`` that prints the keys it was handed, so it reads back what a
real session would start under. The cargo half builds one tiny workspace from
two checkouts into one shared target directory, the lane's shape, and shows a
sibling's build is never reused under per-checkout wrappers — and is under a
shared one, which is what makes the first assertion mean something. Skipped
where ``zsh`` or ``cargo`` is absent.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

SHELL = Path(__file__).resolve().parents[2] / "shell"
INIT = SHELL / "init.zsh"
WRAPPER = SHELL / "rustc-workspace-wrapper"
KEYS = ("CARGO_TARGET_DIR", "RUSTC_WORKSPACE_WRAPPER", "CLIPPY_CONF_DIR")

_PROBE = "; ".join(f'print -r -- "{k}=${{{k}-}}"' for k in KEYS)
#: Every step of `_ds_task_start` that reaches outside the shell is stubbed,
#: so only the launch's own keying runs.
SCRIPT = f"""
source "$1" || exit 9
for fn in _ds_base _ds_secrets _ds_verb_check _ds_substrate_enter \\
  _ds_substrate_write _ds_session_mark _ds_load_record _ds_exit_estimate \\
  _ds_worktree_ensure _ds_op_signout; do
  functions[$fn]=':'
done
_ds_tier() {{ print model; print bedrock; }}
_ds_new_sid() {{ print sid-1; }}
claude() {{ print -- '--in--'; {_PROBE}; }}
cd "$2" || exit 8
{{ eval "$3" }}
print -- '--after--'; {_PROBE}
"""


def sections(stdout: str) -> dict[str, dict[str, str]]:
    out: dict[str, dict[str, str]] = {}
    current = None
    for line in stdout.splitlines():
        if line in ("--in--", "--after--"):
            current = out.setdefault(line.strip("-"), {})
        elif current is not None and "=" in line:
            key, _, value = line.partition("=")
            current[key] = value
    return out


@unittest.skipUnless(shutil.which("zsh"), "needs zsh")
class LaunchKeysTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name).resolve() / "repo"
        self.worktree = self.repo / ".claude" / "worktrees" / "2281"
        for root in (self.repo, self.worktree):
            (root / ".claude" / "shell").mkdir(parents=True)
            shutil.copy(WRAPPER, root / ".claude" / "shell" / WRAPPER.name)
        shutil.copy(INIT, self.repo / ".claude" / "shell" / "init.zsh")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def launch(self, command: str) -> dict[str, dict[str, str]]:
        init = self.repo / ".claude" / "shell" / "init.zsh"
        result = subprocess.run(
            ["zsh", "-fc", SCRIPT, "zsh", str(init), str(self.repo), command],
            capture_output=True,
            text=True,
            env={
                "PATH": "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin",
                "HOME": self.tmp.name,
                "DS_CARGO_LANES": "2",
            },
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return sections(result.stdout)

    def keys_of(self, root: Path, lane: str) -> dict[str, str]:
        return {
            "CARGO_TARGET_DIR": f"{self.tmp.name}/.cargo/target/dropset{lane}",
            "RUSTC_WORKSPACE_WRAPPER": str(root / ".claude" / "shell" / WRAPPER.name),
            "CLIPPY_CONF_DIR": str(root),
        }

    def test_a_task_launch_hands_its_session_the_worktree_s_keys(self) -> None:
        # The launcher runs in the base checkout and `-w` moves inside the CLI,
        # so a launch that skips the keying starts on the base's slots.
        out = self.launch("_ds_task_start 2281 ''")
        self.assertEqual(out["in"], self.keys_of(self.worktree, "-1"))
        self.assertEqual(out["after"], self.keys_of(self.repo, ""))

    def test_a_launch_with_no_worktree_keeps_the_base_keys(self) -> None:
        out = self.launch("_ds_claude_pooled '' model")
        self.assertEqual(out["in"], self.keys_of(self.repo, ""))

    def test_every_worktree_launch_goes_through_the_pooled_launcher(self) -> None:
        # A `-w` or `--resume` launch calling `_ds_claude` directly is the bug
        # this guards: its session would build on the base checkout's slots.
        direct = [
            line.strip()
            for line in INIT.read_text().splitlines()
            if re.search(r"(^|\s)_ds_claude\s", line)
            and re.search(r"\s(-w|--resume)\s", line)
        ]
        self.assertEqual(direct, [])


CRATE_A = "pub struct Level {\n    pub price: u64,\n}\n"
CRATE_B = "pub fn price(l: &a::Level) -> u64 {\n    l.price\n}\n"


@unittest.skipUnless(shutil.which("cargo"), "needs cargo")
class TwoCheckoutsTest(unittest.TestCase):
    """Checkout B breaks crate `a` BEFORE A builds, so B's source is older than
    the shared slot's last build — the case mtime freshness gets wrong."""

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name).resolve()
        for name in "AB":
            ws = self.root / name
            (ws / "Cargo.toml").parent.mkdir(parents=True)
            (ws / "Cargo.toml").write_text(
                '[workspace]\nmembers = ["a", "b"]\nresolver = "2"\n'
            )
            for crate, body, deps in (
                ("a", CRATE_A, ""),
                ("b", CRATE_B, '\n[dependencies]\na = { path = "../a" }\n'),
            ):
                (ws / crate / "src").mkdir(parents=True)
                (ws / crate / "Cargo.toml").write_text(
                    f'[package]\nname = "{crate}"\nversion = "0.1.0"\n'
                    f'edition = "2021"\n{deps}'
                )
                (ws / crate / "src" / "lib.rs").write_text(f"//! {crate}\n{body}")
            shutil.copy(WRAPPER, ws / WRAPPER.name)
        lib = self.root / "B" / "a" / "src" / "lib.rs"
        lib.write_text(lib.read_text().replace("pub price", "pub price_ppm"))
        time.sleep(1.1)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def cargo(self, checkout: str, wrapper_of: str, *args: str) -> int:
        env = {
            k: v
            for k, v in os.environ.items()
            if k not in KEYS and not k.startswith("CARGO_BUILD")
        }
        env["CARGO_TARGET_DIR"] = str(self.root / "target")
        env["RUSTC_WORKSPACE_WRAPPER"] = str(self.root / wrapper_of / WRAPPER.name)
        return subprocess.run(
            ["cargo", *args, "--quiet"],
            cwd=self.root / checkout,
            env=env,
            capture_output=True,
            check=False,
        ).returncode

    def test_each_checkout_builds_its_own_source(self) -> None:
        for args in (["check"], ["test", "--no-run"]):
            with self.subTest(args=args):
                self.assertEqual(self.cargo("A", "A", *args), 0)
                # B's `b` reads a field B's `a` no longer has: it must fail.
                self.assertNotEqual(self.cargo("B", "B", *args), 0)

    def test_a_shared_key_reuses_the_sibling_s_build(self) -> None:
        # The control: B under A's key passes on A's artifacts, never having
        # compiled its own broken source — the false green the key prevents.
        self.assertEqual(self.cargo("A", "A", "check"), 0)
        self.assertEqual(self.cargo("B", "A", "check"), 0)


if __name__ == "__main__":
    unittest.main()
