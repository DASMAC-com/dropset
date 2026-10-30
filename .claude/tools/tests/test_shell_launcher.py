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


#: The fleet knobs every launch requires (the cargo pair while the pool is
#: on); a test unsets one by passing "".
FLEET = {
    "DS_FLEET_TOTAL": "16",
    "DS_FLEET_CPU_LOAD_MAX": "16",
    "DS_CARGO_LANES": "2",
    "CARGO_BUILD_JOBS": "8",
}


def run(command: str, **env: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["zsh", "-fc", SCRIPT, "zsh", str(INIT), command],
        capture_output=True,
        text=True,
        env={"PATH": "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin", **FLEET, **env},
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

    SID = "0f3c2a1b-9d8e-4f70-a1b2-c3d4e5f60718"
    _SID_PROBE = 'claude() { print -r -- "SID=${CLAUDE_SESSION_ID-unset}"; }; '

    def test_the_seeded_or_resumed_id_is_exported_to_the_session(self) -> None:
        for flag in ("--session-id", "--resume"):
            got = run(
                self._SID_PROBE
                + f'_ds_claude "claude-adv-1[1m]" {flag} {self.SID} -n x; '
                + 'print -r -- "AFTER=${CLAUDE_SESSION_ID-unset}"',
            )
            self.assertEqual(got.returncode, 0, got.stderr)
            self.assertIn(f"SID={self.SID}", got.stdout)
            # Function-local: the tab is not left holding it.
            self.assertIn("AFTER=unset", got.stdout)

    def test_a_launch_naming_no_id_blanks_an_inherited_one(self) -> None:
        for args in ("--continue", "--resume eng-12"):
            got = run(
                self._SID_PROBE + f'_ds_claude "claude-adv-1[1m]" {args}',
                CLAUDE_SESSION_ID=self.SID,
            )
            self.assertEqual(got.returncode, 0, got.stderr)
            self.assertIn("SID=\n", got.stdout)

    def _upkeep(self, **env: str) -> Path | None:
        """Run `_ds_upkeep` with the pass, the secrets and the repo stubbed;
        return the attend script's argument log, or None when never called."""
        with tempfile.TemporaryDirectory() as d:
            scripts = Path(d) / ".claude" / "scripts"
            scripts.mkdir(parents=True)
            attend = scripts / "iterm-attend.sh"
            log = Path(d) / "attend.log"
            attend.write_text(f'#!/bin/sh\necho "$@" > "{log}"\n')
            attend.chmod(0o755)
            got = run(
                # `TTY` is zsh's own parameter (empty with no terminal), so it
                # is set in the shell, not passed in the environment.
                f"_ds_secrets() {{ :; }}; python3() {{ :; }}; _DS_REPO='{d}'; "
                "TTY=/dev/ttys999; _ds_upkeep",
                TERM_PROGRAM="iTerm.app",
                **env,
            )
            self.assertEqual(got.returncode, 0, got.stderr)
            return log.read_text().strip() if log.exists() else None

    def test_the_upkeep_tab_is_marked_green_when_the_pass_ends(self) -> None:
        self.assertEqual(self._upkeep(), "--tty /dev/ttys999 --mark")

    def test_iterm_disabled_skips_the_upkeep_mark(self) -> None:
        self.assertIsNone(self._upkeep(DS_ITERM_DISABLED="1"))

    def test_an_unresolved_helper_refuses_a_bedrock_launch(self) -> None:
        tiers = {k: v for k, v in TIERS.items() if k != "DS_MODEL_HELPER"}
        got = run('_ds_claude "claude-adv-1[1m]"', CLAUDE_CODE_USE_BEDROCK="1", **tiers)
        self.assertEqual(got.returncode, 1)
        self.assertNotIn("in", sections(got.stdout))
        self.assertIn("DS_MODEL_HELPER", got.stderr)

    def test_an_unset_fleet_knob_refuses_every_launch_and_names_it(self) -> None:
        for var in FLEET:
            with self.subTest(var=var):
                got = run('_ds_claude "claude-adv-1[1m]"', **TIERS, **{var: ""})
                self.assertEqual(got.returncode, 1)
                self.assertNotIn("in", sections(got.stdout))
                self.assertIn(f"{var} is unset", got.stderr)

    def test_a_malformed_fleet_knob_refuses_as_the_probe_would(self) -> None:
        for var, raw in (
            ("DS_FLEET_TOTAL", " "),
            ("DS_FLEET_TOTAL", "2.5"),
            ("DS_FLEET_CPU_LOAD_MAX", "lots"),
            ("DS_CARGO_LANES", "0"),
            ("CARGO_BUILD_JOBS", "8.5"),
        ):
            with self.subTest(var=var, raw=raw):
                got = run('_ds_claude "claude-adv-1[1m]"', **TIERS, **{var: raw})
                self.assertEqual(got.returncode, 1)
                self.assertNotIn("in", sections(got.stdout))
                self.assertIn(f"{var} ('{raw}') is not", got.stderr)

    def test_the_fleet_knobs_reach_the_session_exported(self) -> None:
        # Plain shell assignments, as a profile might make them: the probe the
        # session runs reads its environment, so the launch must export them.
        got = run(
            "DS_FLEET_TOTAL=12; DS_FLEET_CPU_LOAD_MAX=9.5; DS_CARGO_LANES=3; "
            "CARGO_BUILD_JOBS=6; _ds_claude m; "
            'print -r -- "EXPORTED=$(printenv DS_FLEET_TOTAL)"; '
            'print -r -- "JOBS=$(printenv CARGO_BUILD_JOBS)"',
            **{k: "" for k in FLEET},
        )
        self.assertEqual(got.returncode, 0, got.stderr)
        self.assertIn("EXPORTED=12", got.stdout.splitlines())
        self.assertIn("JOBS=6", got.stdout.splitlines())

    def test_the_cargo_knobs_bind_only_while_the_pool_is_on(self) -> None:
        unset = {"DS_CARGO_LANES": "", "CARGO_BUILD_JOBS": ""}
        refused = run('_ds_claude "claude-adv-1[1m]"', **TIERS, **unset)
        self.assertEqual(refused.returncode, 1)
        self.assertIn("CARGO_BUILD_JOBS is unset", refused.stderr)
        off = run('_ds_claude "claude-adv-1[1m]"', **TIERS, **unset, DS_CARGO_POOL="0")
        self.assertEqual(off.returncode, 0, off.stderr)


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
    def test_architect_refuses_a_stray_word(self) -> None:
        # The retired order, `architect <n> bedrock`, is a plain stray word.
        for command in (
            "architect bedrock",
            "architect bedrock 1572 x",
            "architect 1572 bedrock",
        ):
            with self.subTest(command=command):
                got = run(command)
                self.assertEqual(got.returncode, 1)
                self.assertIn("Usage: architect [anthropic|bedrock] <n>", got.stderr)
                self.assertNotIn("in", sections(got.stdout))

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


@unittest.skipUnless(shutil.which("zsh"), "needs zsh")
class FleetVerbTest(unittest.TestCase):
    """`fleet` hands each accepted form to the resume tool, and only those."""

    STUBS = (
        "_ds_base() { :; }; _ds_secrets() { :; }; _ds_op_signout() { :; }; "
        'python3() { print -r -- "tool ${@:2}"; }; '
    )

    def test_each_form_reaches_the_tool_with_its_flags(self) -> None:
        for words, flags in (
            ("", ""),
            ("go", "--apply"),
            ("go --over-budget", "--apply --over-budget"),
        ):
            with self.subTest(words=words):
                got = run(self.STUBS + f"fleet {words}")
                self.assertEqual(got.returncode, 0, got.stderr)
                self.assertIn(f"tool {flags}\n", got.stdout)

    def test_a_stray_word_is_refused(self) -> None:
        for words in ("go --over", "go --over-budget now", "stop"):
            with self.subTest(words=words):
                got = run(self.STUBS + f"fleet {words}")
                self.assertEqual(got.returncode, 1)
                self.assertNotIn("tool", got.stdout)
                self.assertIn("Usage: fleet [go [--over-budget]]", got.stderr)


@unittest.skipUnless(shutil.which("zsh"), "needs zsh")
class ClassStampTest(unittest.TestCase):
    """The launcher's class record names the pid that runs `claude`."""

    def _launch(
        self, key: str, cls: str, recorded: str | None = None
    ) -> tuple[str, str]:
        with tempfile.TemporaryDirectory() as state:
            if recorded is not None:
                Path(state, f"{key}.class").write_text(recorded)
            got = run(
                f'_DS_SUBSTRATE_DIR={state}; _ds_class_key={key}; _ds_class="{cls}"; '
                'claude() { zmodload zsh/system; print -r -- "pid=$sysparams[pid]"; }; '
                "_ds_claude m"
            )
            self.assertEqual(got.returncode, 0, got.stderr)
            pid = got.stdout.split("pid=")[1].split()[0]
            return Path(state, f"{key}.class").read_text().strip(), pid

    def test_a_fixed_class_is_stamped_with_claude_s_pid(self) -> None:
        record, pid = self._launch("2258", "building", recorded="free\n")
        self.assertEqual(record, f"building {pid}")

    def test_an_empty_class_keeps_the_recorded_one(self) -> None:
        record, pid = self._launch("2240", "", recorded="free\n")
        self.assertEqual(record, f"free {pid}")

    def test_an_empty_class_with_no_record_is_building(self) -> None:
        record, pid = self._launch("sessions/abc", "")
        self.assertEqual(record, f"building {pid}")

    def test_an_executable_claude_keeps_the_stamped_pid(self) -> None:
        # The real path: the subshell `exec`s, so the pid it stamped is the
        # one the probe will find running `claude`.
        with (
            tempfile.TemporaryDirectory() as state,
            tempfile.TemporaryDirectory() as bin_dir,
        ):
            stub = Path(bin_dir, "claude")
            stub.write_text('#!/bin/sh\necho "pid=$$"\n')
            stub.chmod(0o755)
            got = subprocess.run(
                [
                    "zsh",
                    "-fc",
                    f'source "$1" || exit 9; _DS_SUBSTRATE_DIR={state}; '
                    "_ds_class_key=7; _ds_class=free; _ds_claude m",
                    "zsh",
                    str(INIT),
                ],
                capture_output=True,
                text=True,
                env={"PATH": f"{bin_dir}:/usr/bin:/bin", **FLEET},
                check=False,
            )
            self.assertEqual(got.returncode, 0, got.stderr)
            pid = got.stdout.split("pid=")[1].split()[0]
            self.assertEqual(Path(state, "7.class").read_text().strip(), f"free {pid}")


if __name__ == "__main__":
    unittest.main()
