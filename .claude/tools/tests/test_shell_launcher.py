"""Tests for the shared launcher ``_ds_claude`` and the launch-line grammar in
``.claude/shell/init.zsh``.

``claude`` is replaced by a shell function that prints the environment it was
handed, so each test reads back exactly what a real session would start under,
and nothing is launched. Skipped where ``zsh`` is absent.
"""

from __future__ import annotations

import shutil
import subprocess
import tempfile
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


#: A stub `op` and `claude` that append each call to `$OP_LOG`, in order; the
#: reads run in a `$(…)` substitution, so a file is the one shared record.
OP_STUBS = """
op() {
  print -r -- "op $1 ${@[-1]}" >> "$OP_LOG"
  [[ "$1" == read ]] && print -r -- secret
  [[ "$1" != signout || -z "$OP_SIGNOUT_FAILS" ]]
}
claude() { print -r -- claude >> "$OP_LOG"; }
"""

OP_REFS = {
    "DS_OP_ACCOUNT": "acct.example.com",
    "DS_OP_LINEAR_REF": "op://vault/linear/credential",
    "DS_OP_GITHUB_REF": "op://vault/github/credential",
    "DS_OP_BEDROCK_REF": "op://vault/bedrock/credential",
}


@unittest.skipUnless(shutil.which("zsh"), "needs zsh")
class OpSignoutTest(unittest.TestCase):
    def launch(
        self, command: str, **env: str
    ) -> tuple[subprocess.CompletedProcess, list[str]]:
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "op.log"
            log.touch()
            got = run(OP_STUBS + command, OP_LOG=str(log), **OP_REFS, **TIERS, **env)
            return got, log.read_text().splitlines()

    def test_a_bedrock_launch_signs_out_once_after_every_read(self) -> None:
        # Chained helpers re-run `_ds_secrets`; the reads still happen once.
        got, calls = self.launch(
            '_ds_secrets; _ds_bedrock_env; _ds_secrets; _ds_claude "claude-adv-1[1m]"'
        )
        self.assertEqual(got.returncode, 0, got.stderr)
        self.assertEqual(
            calls,
            [
                "op read op://vault/linear/credential",
                "op read op://vault/github/credential",
                "op read op://vault/bedrock/credential",
                "op signout acct.example.com",
                "claude",
            ],
        )

    def test_no_read_means_no_signout(self) -> None:
        got, calls = self.launch(
            '_ds_secrets; _ds_claude "claude-adv-1[1m]"',
            LINEAR_API_KEY="pinned",
            GITHUB_MCP_PAT="pinned",
        )
        self.assertEqual(got.returncode, 0, got.stderr)
        self.assertEqual(calls, ["claude"])

    def test_a_failed_signout_warns_and_still_launches(self) -> None:
        got, calls = self.launch(
            '_ds_secrets; _ds_claude "claude-adv-1[1m]"', OP_SIGNOUT_FAILS="1"
        )
        self.assertEqual(got.returncode, 0, got.stderr)
        self.assertEqual(calls[-1], "claude")
        self.assertIn("op signout failed", got.stderr)

    def test_a_second_launch_in_the_shell_signs_out_only_after_a_new_read(
        self,
    ) -> None:
        got, calls = self.launch(
            "_ds_secrets; _ds_claude m; _ds_secrets; _ds_claude m; "
            "unset LINEAR_API_KEY; _ds_secrets; _ds_claude m"
        )
        self.assertEqual(got.returncode, 0, got.stderr)
        self.assertEqual(
            calls,
            [
                "op read op://vault/linear/credential",
                "op read op://vault/github/credential",
                "op signout acct.example.com",
                "claude",
                "claude",
                "op read op://vault/linear/credential",
                "op signout acct.example.com",
                "claude",
            ],
        )

    def test_a_key_set_without_export_is_still_exported(self) -> None:
        got, calls = self.launch(
            "LINEAR_API_KEY=x; GITHUB_MCP_PAT=y; _ds_secrets; "
            'print -r -- "type=${(t)LINEAR_API_KEY} ${(t)GITHUB_MCP_PAT}"'
        )
        self.assertEqual(got.returncode, 0, got.stderr)
        self.assertIn("type=scalar-export scalar-export", got.stdout)
        self.assertEqual(calls, [])


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

    def test_a_new_session_reaches_the_title_guard_with_its_launch_line(self) -> None:
        """Each issue verb parses its word-first form and hands the guard the
        normalized launch line. The stub refuses, so nothing past the guard
        runs and no Bedrock export is entered."""
        stub = '_ds_verb_check() { print -u2 "guard: $*"; return 1; }; '
        with tempfile.TemporaryDirectory() as home:
            env = {"CLAUDE_CONFIG_DIR": home, "DS_MODEL_ADVISOR": "claude-adv-1[1m]"}
            env.update(TIERS)
            for command, line in (
                ("explore bedrock eng-999991", "explore bedrock 999991"),
                ("explore anthropic 999991", "explore anthropic 999991"),
                ("architect bedrock 999992", "architect bedrock 999992"),
                ("architect anthropic eng-999992", "architect anthropic 999992"),
                ("chore bedrock 999993", "chore bedrock 999993"),
            ):
                with self.subTest(command=command):
                    got = run(stub + command, **env)
                    self.assertEqual(got.returncode, 1, got.stderr)
                    self.assertIn(f"guard: {line}", got.stderr)
                    self.assertNotIn("in", sections(got.stdout))

    def test_explore_refuses_a_word_as_its_name(self) -> None:
        for command in ("explore bedrock", "explore bedrock anthropic"):
            with self.subTest(command=command):
                got = run(command)
                self.assertEqual(got.returncode, 1)
                self.assertIn("Usage: explore [anthropic|bedrock]", got.stderr)


if __name__ == "__main__":
    unittest.main()
