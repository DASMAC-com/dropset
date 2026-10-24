"""Tests for the shared launcher ``_ds_claude`` and the launch-line grammar in
``.claude/shell/init.zsh``.

``claude`` is replaced by a shell function that prints the environment it was
handed, so each test reads back exactly what a real session would start under,
and nothing is launched. Skipped where ``zsh`` is absent.
"""

from __future__ import annotations

import shutil
import subprocess
import unittest
from pathlib import Path

INIT = Path(__file__).resolve().parents[2] / "shell" / "init.zsh"

SLOTS = (
    "ANTHROPIC_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
)

#: A stub `claude` that prints each slot it received, then the same read back
#: in the calling shell after the launch returns.
_PROBE = "; ".join(f'print -r -- "{s}=${{{s}-}}"' for s in SLOTS)
SCRIPT = f"""
source "$1" || exit 9
claude() {{ print -- '--in--'; {_PROBE}; }}
{{ eval "$2" }}
rc=$?
print -- '--after--'; {_PROBE}
exit $rc
"""

TIERS = {
    "DS_MODEL_EXECUTOR": "claude-exec-1[1m]",
    "DS_MODEL_HELPER": "claude-help-1[1m]",
    "DS_MODEL_BACKGROUND": "claude-bg-1",
}


def run(command: str, **env: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["zsh", "-fc", SCRIPT, "zsh", str(INIT), command],
        capture_output=True,
        text=True,
        env={"PATH": "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin", **env},
        check=False,
    )


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
class SharedLauncherTest(unittest.TestCase):
    def test_a_bedrock_launch_binds_every_slot_to_a_tier(self) -> None:
        got = run('_ds_claude "claude-adv-1[1m]"', CLAUDE_CODE_USE_BEDROCK="1", **TIERS)
        self.assertEqual(got.returncode, 0, got.stderr)
        seen = sections(got.stdout)["in"]
        self.assertEqual(
            seen,
            {
                "ANTHROPIC_MODEL": "claude-adv-1[1m]",
                "CLAUDE_CODE_SUBAGENT_MODEL": "claude-exec-1[1m]",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-exec-1[1m]",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-help-1[1m]",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-bg-1",
            },
        )

    def test_no_slot_outlives_the_command(self) -> None:
        got = run('_ds_claude "claude-adv-1[1m]"', CLAUDE_CODE_USE_BEDROCK="1", **TIERS)
        self.assertEqual(set(sections(got.stdout)["after"].values()), {""})

    def test_a_subscription_launch_binds_only_the_session_model(self) -> None:
        got = run('_ds_claude "claude-adv-1[1m]"', **TIERS)
        seen = sections(got.stdout)["in"]
        self.assertEqual(seen.pop("ANTHROPIC_MODEL"), "claude-adv-1[1m]")
        self.assertEqual(set(seen.values()), {""})

    def test_an_unresolved_helper_refuses_a_bedrock_launch(self) -> None:
        tiers = {k: v for k, v in TIERS.items() if k != "DS_MODEL_HELPER"}
        got = run('_ds_claude "claude-adv-1[1m]"', CLAUDE_CODE_USE_BEDROCK="1", **tiers)
        self.assertEqual(got.returncode, 1)
        self.assertNotIn("in", sections(got.stdout))
        self.assertIn("DS_MODEL_HELPER", got.stderr)


@unittest.skipUnless(shutil.which("zsh"), "needs zsh")
class LaunchLineGrammarTest(unittest.TestCase):
    def test_architect_s_retired_order_names_the_new_one(self) -> None:
        got = run("architect 1572 bedrock")
        self.assertEqual(got.returncode, 1)
        self.assertIn("`architect bedrock 1572`", got.stderr)
        self.assertNotIn("in", sections(got.stdout))

    def test_architect_refuses_a_stray_word(self) -> None:
        for command in ("architect bedrock", "architect bedrock 1572 x"):
            with self.subTest(command=command):
                got = run(command)
                self.assertEqual(got.returncode, 1)
                self.assertIn("Usage: architect [anthropic|bedrock] <n>", got.stderr)

    def test_explore_refuses_a_word_as_its_name(self) -> None:
        for command in ("explore bedrock", "explore bedrock anthropic"):
            with self.subTest(command=command):
                got = run(command)
                self.assertEqual(got.returncode, 1)
                self.assertIn("Usage: explore [anthropic|bedrock]", got.stderr)


if __name__ == "__main__":
    unittest.main()
