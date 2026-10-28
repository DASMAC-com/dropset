#!/usr/bin/env python3
"""Behavior tests for the session verbs' SUBSTRATE machinery (stdlib unittest).

Three ratified properties are asserted here, each of which fails silently in
production if it regresses — which is the whole reason they are tested rather
than reviewed:

  * Each verb launches its tier's model, delivered as a command-scoped
    `ANTHROPIC_MODEL`, and a config that does not resolve launches nothing.
    The fallbacks keep the `[1m]` suffix: Bedrock defaults an unsuffixed id to
    the 200k window and reports nothing, so losing it looks exactly like a
    session that filled up.
  * An absent marker reads as `anthropic`. Every session predating markers is
    a seat session, and the conservative error is spending the subscription
    window rather than spending credits on something unintended.
  * A seat verb CLEARS inherited Bedrock exports. The helpers export into the
    calling shell (they must — a child process could not set what `claude`
    inherits), so a tab that ran `task` stays a Bedrock tab afterwards, and the
    seat pin is expressed as the ABSENCE of `CLAUDE_CODE_USE_BEDROCK`.

Landing as Python driving real `zsh`, for the same reason `test_ds_pull.py`
does: `make tools-tests` already discovers `test_*.py` under `.claude/scripts`
and the repo has no shell test runner. The shell-side branches ARE the
substance, so porting the logic to Python and testing that instead would test
the easy half.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

INIT = Path(__file__).resolve().parents[1] / "shell" / "init.zsh"

# No case may reach a real iTerm tab, whatever it forgets to stub: inherited by
# every zsh this spawns (`iterm_api.DISABLED_ENV`).
os.environ["DS_ITERM_DISABLED"] = "1"

# The helpers are zsh and deliberately so (`<->` all-digit tests, `>|` clobber),
# and the Linux CI runner ships no zsh: without this guard every case fails with
# FileNotFoundError, which says nothing about the code under test.
_NEEDS_ZSH = "the session helpers are zsh; no zsh on this machine"

#: Cleared from the inherited environment so a case tests the helper rather than
#: the operator's own shell profile — several of these are exactly what an
#: operator running Bedrock sessions has exported.
_SUITE_OWNED_ENV = (
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_MODEL",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_REGION",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_USE_BEDROCK",
    "DS_BEDROCK_FAST_MODEL",
    "DS_BEDROCK_MODEL",
    "DS_BEDROCK_PROBE",
    "DS_BEDROCK_REGION",
    "DS_MODEL_BACKGROUND",
    "DS_MODEL_ADVISOR",
    "DS_MODEL_ADVISOR_SUBSTRATE",
    "DS_MODEL_EXECUTOR",
    "DS_MODEL_EXECUTOR_SUBSTRATE",
    "DS_MODEL_HELPER",
    "DS_MODEL_HELPER_SUBSTRATE",
    "DS_OP_ACCOUNT",
    "DS_OP_BEDROCK_REF",
    "ENABLE_PROMPT_CACHING_1H",
    "_DS_REPO",
)


@unittest.skipUnless(shutil.which("zsh"), _NEEDS_ZSH)
class SubstrateHarness(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.repo = Path(self._tmp.name) / "repo"
        (self.repo / ".claude").mkdir(parents=True)
        self.claude_home = Path(self._tmp.name) / "claude-home"

    def _zsh(self, body, *, env=None):
        """Source the real init against a scratch `_DS_REPO`, then run `body`."""
        script = (
            f'source "{INIT}" 2>/dev/null; '
            f'_DS_REPO="{self.repo}"; '
            f'_DS_SUBSTRATE_DIR="{self.repo}/.claude/session-substrate"; '
            f"{body}"
        )
        child_env = {**os.environ}
        for name in _SUITE_OWNED_ENV:
            child_env.pop(name, None)
        # `_ds_session` globs the projects tree for a session id, so a case must
        # never see the operator's real transcripts.
        child_env["CLAUDE_CONFIG_DIR"] = str(self.claude_home)
        child_env.update(env or {})
        return subprocess.run(
            ["zsh", "-c", script],
            capture_output=True,
            text=True,
            check=False,
            env=child_env,
        )


#: A runtime config with every tier set. Placeholder ids, never real ones: the
#: repo pins no model, and a test that did would need editing every release.
_CONFIG = {
    "DS_MODEL_ADVISOR": "judge-model[1m]",
    "DS_MODEL_EXECUTOR": "work-model[1m]",
    "DS_MODEL_HELPER": "help-model[1m]",
    "DS_MODEL_BACKGROUND": "bg-profile-id",
}


class TierResolution(SubstrateHarness):
    """`_ds_tier` — the role → model + substrate table, and its refusals."""

    def _tier(self, tier, override="", env=None):
        result = self._zsh(
            f"_ds_tier {tier} '{override}'", env={**_CONFIG, **(env or {})}
        )
        return result, result.stdout.splitlines()

    def test_the_default_substrates(self):
        # Advisor defaults to the subscription, executor to Bedrock — the
        # ratified role mapping.
        result, lines = self._tier("advisor")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(lines, ["judge-model[1m]", "anthropic"])
        result, lines = self._tier("executor")
        self.assertEqual(lines, ["work-model[1m]", "bedrock"])
        result, lines = self._tier("helper")
        self.assertEqual(lines, ["help-model[1m]", "bedrock"])

    def test_an_unset_model_refuses_and_names_the_variable(self):
        # No committed fallback: an unset tier must not launch on some id the
        # repo carried, which is the generation-stale slip the tiers retire.
        for tier, var in (
            ("advisor", "DS_MODEL_ADVISOR"),
            ("executor", "DS_MODEL_EXECUTOR"),
            ("helper", "DS_MODEL_HELPER"),
        ):
            with self.subTest(tier=tier):
                result, lines = self._tier(tier, env={var: ""})
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(lines, [])
                self.assertIn(f"{var} is unset", result.stderr)

    def test_the_configured_model_and_substrate_win_verbatim(self):
        # The runtime config is the one place a new model lands, so a
        # configured string must reach the launch untouched.
        result, lines = self._tier(
            "executor",
            env={
                "DS_MODEL_EXECUTOR": "next-model[1m]",
                "DS_MODEL_EXECUTOR_SUBSTRATE": "anthropic",
            },
        )
        self.assertEqual(lines, ["next-model[1m]", "anthropic"])

    def test_the_override_beats_the_configured_substrate(self):
        # `architect <n> bedrock`: one word, no config edit.
        result, lines = self._tier(
            "advisor", "bedrock", env={"DS_MODEL_ADVISOR_SUBSTRATE": "anthropic"}
        )
        self.assertEqual(lines, ["judge-model[1m]", "bedrock"])

    def test_an_unknown_substrate_refuses_to_launch(self):
        # The pre-session half of fail-fast: refuse offline, before a session
        # starts and fails on its first turn.
        result, lines = self._tier(
            "advisor", env={"DS_MODEL_ADVISOR_SUBSTRATE": "local"}
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(lines, [])
        self.assertIn("must be anthropic or bedrock", result.stderr)

    def test_a_model_with_whitespace_refuses_to_launch(self):
        result, lines = self._tier("executor", env={"DS_MODEL_EXECUTOR": "claude opus"})
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(lines, [])
        # The reason, so an undefined function or a syntax error cannot pass.
        self.assertIn("is not a model id", result.stderr)

    def test_an_unknown_tier_refuses(self):
        result, lines = self._tier("frontier")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(lines, [])
        self.assertIn("unknown model tier", result.stderr)

    def test_config_escapes_are_echoed_literally(self):
        # The refusal echoes operator config, so `print -r`: no escape expands.
        result, _ = self._tier(
            "executor", env={"DS_MODEL_EXECUTOR_SUBSTRATE": "x\\tbad"}
        )
        self.assertIn("x\\tbad", result.stderr)

    def test_an_unsuffixed_model_on_bedrock_warns_but_is_honored(self):
        # Warn, never refuse: the override is the operator's to make.
        result, lines = self._tier(
            "executor", env={"DS_MODEL_EXECUTOR": "us.anthropic.some-model"}
        )
        self.assertEqual(lines, ["us.anthropic.some-model", "bedrock"])
        self.assertIn("no context-window", result.stderr)

    def test_a_suffixed_model_is_silent(self):
        # The positive assertion keeps the `assertNotIn` from passing
        # vacuously when the function emits nothing at all.
        result, lines = self._tier(
            "executor", env={"DS_MODEL_EXECUTOR": "work-model[200k]"}
        )
        self.assertEqual(lines, ["work-model[200k]", "bedrock"])
        self.assertNotIn("no context-window", result.stderr)

    def test_the_retired_bedrock_spelling_is_named(self):
        # A config still on the old name gets told the new one.
        result, lines = self._tier(
            "executor", env={"DS_MODEL_EXECUTOR": "", "DS_BEDROCK_MODEL": "x[1m]"}
        )
        self.assertEqual(lines, [])
        self.assertIn("DS_BEDROCK_MODEL is retired", result.stderr)


class VerbLaunch(SubstrateHarness):
    """End-to-end through each verb, with `claude` stubbed to report its env.

    The model is delivered as a command-scoped `ANTHROPIC_MODEL`, so the stub
    sees it and the shell afterwards must not — that is the no-leak property.
    """

    _STUBS = (
        'claude() { print -r -- "MODEL=${ANTHROPIC_MODEL-unset}'
        " USE=${CLAUDE_CODE_USE_BEDROCK-unset}"
        # Through `printenv`, a child process, so only an EXPORT is seen.
        " SUB=$(printenv CLAUDE_CODE_SUBAGENT_MODEL || print unset)"
        " SONNET=$(printenv ANTHROPIC_DEFAULT_SONNET_MODEL || print unset)"
        ' ARGS=$*"; }; '
        "_ds_base() { :; }; _ds_secrets() { :; }; _ds_aws_login() { :; }; "
        "_ds_pull() { :; }; "
        # A warm day: these cases test the scheduler's own launch, not the
        # morning pre-launch (`test_plan_cycle_loop.PreLaunch`).
        "_ds_plan_fresh_day() { return 1; }; "
        # The title guard reads Linear; its outcomes are `test_verb_check.py`'s.
        "_ds_verb_check() { :; }; "
        # The model probe's transport: argv and stdin logged apart, so a case
        # can tell where the token went; the reply is the env's to choose.
        'curl() { print -r -- "ARGV=$* STDIN=$(cat)" >> "$_DS_REPO/curl.log"; '
        'printf \'%s\\n%s\' "${PROBE_BODY:-ok}" "${PROBE_CODE:-200}"; }; '
    )
    _AFTER = '; print -r -- "RC=$?"; print -r -- "AFTER=${ANTHROPIC_MODEL-unset}"'
    _TOKEN = {"AWS_BEARER_TOKEN_BEDROCK": "placeholder-key"}

    def _launch(self, verb, env=None):
        result = self._zsh(
            self._STUBS + verb + self._AFTER, env={**_CONFIG, **(env or {})}
        )
        return result, result.stdout

    def test_plan_launches_the_advisor_tier_on_bedrock(self):
        # Whatever the advisor config says: that governs `architect` alone.
        result, out = self._launch(
            "plan", env={**self._TOKEN, "DS_MODEL_ADVISOR_SUBSTRATE": "anthropic"}
        )
        self.assertIn("MODEL=judge-model[1m] USE=1", out, result.stderr)
        # No `--model` flag: the environment is the one delivery mechanism.
        self.assertNotIn("--model", out)
        self.assertIn("AFTER=unset", out)
        result, out = self._launch("plan bedrock", env=self._TOKEN)
        self.assertIn("MODEL=judge-model[1m] USE=1", out, result.stderr)

    def test_plan_anthropic_is_the_seat_override(self):
        result, out = self._launch("plan anthropic", env=self._TOKEN)
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)

    def test_plan_rejects_any_other_word(self):
        result, out = self._launch("plan local")
        self.assertIn("RC=1", out)
        self.assertNotIn("MODEL=", out)

    def _plan_transcript(self):
        """Write a transcript for today's first `plan` id, so the launch reopens."""
        return (
            "sid=$(_ds_daily_sid plan 0 $(date +%Y%m%d)); "
            'mkdir -p "$CLAUDE_CONFIG_DIR/projects/some-slug"; '
            ': >| "$CLAUDE_CONFIG_DIR/projects/some-slug/$sid.jsonl"; '
        )

    def test_a_plan_reopen_lands_on_the_mornings_substrate(self):
        # The morning's `plan anthropic` recorded the seat; a bare reopen must
        # not drift onto the Bedrock default mid-day.
        self._launch("plan anthropic")
        result, out = self._launch(self._plan_transcript() + "plan", env=self._TOKEN)
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)
        self.assertIn("--resume", out)

    def test_a_plan_reopen_warns_and_keeps_a_mismatched_word(self):
        self._launch("plan", env=self._TOKEN)
        result, out = self._launch(
            self._plan_transcript() + "plan anthropic", env=self._TOKEN
        )
        self.assertIn("MODEL=judge-model[1m] USE=1", out, result.stderr)
        self.assertIn("recorded on bedrock", result.stderr)

    def test_an_unrecorded_plan_reopen_reads_as_anthropic(self):
        # A session opened before the record existed was a seat session.
        result, out = self._launch(self._plan_transcript() + "plan", env=self._TOKEN)
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)
        self.assertIn("no substrate record", result.stderr)

    # The file count rides along, so two records holding one word cannot pass.
    # A launch's `<id>.class` sidecar (the machine-load class record) sits
    # beside the substrate record and is not one.
    _SESSIONS = (
        "files=($_DS_SUBSTRATE_DIR/sessions/*(N)); files=(${files:#*.class}); "
        'print -r -- "SESSIONS=${#files} $(cat $files)"'
    )

    def test_every_launch_path_records_its_session_once(self):
        # `_ds_session` (plan, architect, explore, chore) and `_ds_task_start`.
        for verb, word in (
            ("plan", "bedrock"),
            ("plan anthropic", "anthropic"),
            ("architect 12", "anthropic"),
            ("explore 12", "bedrock"),
            ("chore 12", "bedrock"),
            ("task 7", "bedrock"),
            ("task anthropic 7", "anthropic"),
        ):
            with self.subTest(verb=verb):
                result, out = self._launch(
                    f"rm -rf $_DS_SUBSTRATE_DIR; {verb}; {self._SESSIONS}",
                    env=self._TOKEN,
                )
                self.assertIn(f"SESSIONS=1 {word}\n", out, result.stderr)

    def test_a_task_session_id_is_passed_and_recorded(self):
        result, out = self._launch(
            "task 7; ls $_DS_SUBSTRATE_DIR/sessions", env=self._TOKEN
        )
        sid = out.splitlines()[-3]
        self.assertRegex(sid, r"^[0-9a-f-]{36}$")
        self.assertIn(f"--session-id {sid} ", out, result.stderr)

    def test_a_task_with_no_id_source_launches_nothing(self):
        result, out = self._launch(
            "_ds_new_sid() { :; }; task 7; "
            'print -r -- "TASK_RC=$? USE=${CLAUDE_CODE_USE_BEDROCK-unset}"; '
            # A refusal must not flip an earlier session's tag marker.
            "[[ -e $_DS_SUBSTRATE_DIR/7 ]] && print -r -- MARKED",
            env=self._TOKEN,
        )
        self.assertNotIn("MODEL=", out)
        self.assertNotIn("MARKED", out)
        self.assertIn("TASK_RC=1", out)
        self.assertIn("USE=unset", out)
        self.assertIn("could not generate a session id", result.stderr)

    def test_a_resume_never_rewrites_the_session_record(self):
        # A value no launch writes: a rewrite of any kind would replace it.
        result, out = self._launch(
            self._plan_transcript()
            + 'mkdir -p "$_DS_SUBSTRATE_DIR/sessions"; '
            + 'print -r -- odd >| "$_DS_SUBSTRATE_DIR/sessions/$sid"; '
            + f"plan; {self._SESSIONS}",
            env=self._TOKEN,
        )
        self.assertIn("--resume", out, result.stderr)
        self.assertIn("SESSIONS=1 odd\n", out)

    def test_a_create_that_never_started_does_not_pin_its_substrate(self):
        # A Bedrock `plan` that died before writing a transcript, then a
        # `plan anthropic`: the session that ran is the seat one, so the
        # record must say so, or every reopen moves it onto Bedrock.
        self._launch("plan", env=self._TOKEN)
        result, out = self._launch(f"plan anthropic; {self._SESSIONS}")
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)
        self.assertIn("SESSIONS=1 anthropic\n", out)

    def test_a_reopen_reads_the_current_cycles_record(self):
        # After a cycle the day's live session is cycle 1, not cycle 0.
        result, out = self._launch(
            "day=$(date +%Y%m%d); mkdir -p $_DS_SUBSTRATE_DIR/sessions; "
            "print -r -- 1 >| $_DS_SUBSTRATE_DIR/plan-cycle-$day; "
            "sid=$(_ds_daily_sid plan 1 $day); "
            "print -r -- anthropic >| $_DS_SUBSTRATE_DIR/sessions/$sid; "
            'mkdir -p "$CLAUDE_CONFIG_DIR/projects/some-slug"; '
            ': >| "$CLAUDE_CONFIG_DIR/projects/some-slug/$sid.jsonl"; '
            'plan; print -r -- "SID=$sid"',
            env=self._TOKEN,
        )
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)
        sid = out.split("SID=")[1].splitlines()[0]
        self.assertIn(f"--resume {sid} ", out)

    def test_the_plan_cycle_reads_padded_and_garbage_counters(self):
        result = self._zsh(
            "f=$_DS_REPO/c; _ds_plan_cycle $f; print -r -- 01 >| $f; "
            "_ds_plan_cycle $f; print -r -- x >| $f; _ds_plan_cycle $f"
        )
        self.assertEqual(result.stdout.split(), ["0", "1", "0"], result.stderr)

    def test_a_plan_after_a_task_in_the_same_tab_is_not_on_bedrock(self):
        # The inherited Bedrock pin must lose to the verb's own tier.
        result, out = self._launch("task 7; plan anthropic", env=self._TOKEN)
        lines = [ln for ln in out.splitlines() if ln.startswith("MODEL=")]
        self.assertEqual(len(lines), 2, out)
        self.assertTrue(lines[0].startswith("MODEL=work-model[1m] USE=1"))
        self.assertTrue(lines[1].startswith("MODEL=judge-model[1m] USE=unset"))

    def test_task_launches_the_executor_tier_on_bedrock(self):
        result, out = self._launch("task 7", env=self._TOKEN)
        self.assertIn("MODEL=work-model[1m] USE=1", out, result.stderr)
        # The bare number names the worktree and the session alike.
        self.assertIn("-w 7 -n 7", out)

    def test_task_anthropic_and_the_retired_local_alias(self):
        # The marker the launch writes is read raw: `_ds_substrate_read` would
        # map a stale `seat` to anthropic too, so it cannot tell them apart.
        marker = 'print -r -- "MARK=$(<$_DS_SUBSTRATE_DIR/7)"'
        result, out = self._launch(f"task anthropic 7; {marker}")
        self.assertIn("MODEL=work-model[1m] USE=unset", out, result.stderr)
        self.assertIn("MARK=anthropic", out)
        result, out = self._launch("task local 7")
        self.assertIn("MODEL=work-model[1m] USE=unset", out)
        self.assertIn("`local` is retired", result.stderr)

    def test_task_bedrock_overrides_an_anthropic_executor_config(self):
        result, out = self._launch(
            "task bedrock 7",
            env={**self._TOKEN, "DS_MODEL_EXECUTOR_SUBSTRATE": "anthropic"},
        )
        self.assertIn("MODEL=work-model[1m] USE=1", out, result.stderr)

    def test_a_bad_config_launches_nothing(self):
        # With a token, so a missing-token refusal cannot satisfy it instead.
        result, out = self._launch(
            "task 7", env={**self._TOKEN, "DS_MODEL_EXECUTOR_SUBSTRATE": "seat"}
        )
        self.assertIn("RC=1", out)
        self.assertNotIn("MODEL=", out)
        self.assertIn("must be anthropic or bedrock", result.stderr)

    _MARK = (
        'print -r -- "MARK=$(<$_DS_SUBSTRATE_DIR/{0}) $(<$_DS_SUBSTRATE_DIR/{0}.tier)"'
    )

    def test_explore_is_bedrock_whatever_the_advisor_config(self):
        # An audit's sub-agent fan-out is what moved it off the seat, so the
        # advisor substrate `plan` and `architect` read must not move it back.
        result, out = self._launch(
            "explore 12; " + self._MARK.format(12),
            env={**self._TOKEN, "DS_MODEL_ADVISOR_SUBSTRATE": "anthropic"},
        )
        self.assertIn(
            "MODEL=judge-model[1m] USE=1 SUB=work-model[1m] SONNET=help-model[1m]",
            out,
            result.stderr,
        )
        self.assertIn("-n 12 --permission-mode auto -w 12", out)
        self.assertIn("MARK=bedrock advisor", out)
        # Scoped to the launch: neither outlives it into the tab.
        result, out = self._launch(
            'explore 12; print -r -- "LEAK=${CLAUDE_CODE_SUBAGENT_MODEL-unset}'
            '/${ANTHROPIC_DEFAULT_SONNET_MODEL-unset}"',
            env=self._TOKEN,
        )
        self.assertIn("LEAK=unset/unset", out, result.stderr)
        # The prefixed spelling names the same issue, worktree and session.
        result, out = self._launch("explore eng-12", env=self._TOKEN)
        self.assertIn("-n 12 --permission-mode auto -w 12", out, result.stderr)

    def test_explore_anthropic_is_the_seat_with_no_tier_exports(self):
        result, out = self._launch(
            "explore anthropic 12; " + self._MARK.format(12), env=self._TOKEN
        )
        self.assertIn(
            "MODEL=judge-model[1m] USE=unset SUB=unset SONNET=unset",
            out,
            result.stderr,
        )
        self.assertIn("MARK=anthropic advisor", out)

    def test_a_bedrock_explore_refuses_an_unset_sub_agent_tier(self):
        # Unpinned, every sub-agent would silently inherit the advisor model.
        for var in ("DS_MODEL_EXECUTOR", "DS_MODEL_HELPER"):
            with self.subTest(var=var):
                result, out = self._launch("explore 12", env={**self._TOKEN, var: ""})
                self.assertIn("RC=1", out)
                self.assertNotIn("MODEL=", out)
                self.assertIn(f"{var} is unset", result.stderr)

    def _explore_transcript(self, raw):
        """Write a transcript for `explore <raw>`'s id, so the launch reopens."""
        return (
            f"sid=$(_ds_topic_sid explore {raw}); "
            'mkdir -p "$CLAUDE_CONFIG_DIR/projects/some-slug"; '
            ': >| "$CLAUDE_CONFIG_DIR/projects/some-slug/$sid.jsonl"; '
        )

    def test_an_explore_reopen_keeps_its_recorded_substrate(self):
        # Unmarked: a session from before the Bedrock default, so the seat.
        result, out = self._launch(
            self._explore_transcript(12) + "explore 12", env=self._TOKEN
        )
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)
        self.assertIn("ARGS=--resume", out)
        # Marked bedrock: back on Bedrock, sub-agent pins included.
        result, out = self._launch(
            self._explore_transcript(13)
            + "_ds_substrate_write 13 bedrock advisor; explore 13",
            env=self._TOKEN,
        )
        self.assertIn("USE=1 SUB=work-model[1m]", out, result.stderr)

    def test_an_explore_reopen_refuses_a_word_that_switches_provider(self):
        result, out = self._launch(
            self._explore_transcript(12)
            + "_ds_substrate_write 12 bedrock advisor; explore anthropic 12",
            env=self._TOKEN,
        )
        self.assertIn("RC=1", out)
        self.assertNotIn("MODEL=", out)
        self.assertIn("keeps its substrate", result.stderr)

    def test_free_form_explore_keeps_its_exp_name_and_is_marked(self):
        result, out = self._launch(
            "explore pricing-survey; " + self._MARK.format("exp-pricing-survey"),
            env=self._TOKEN,
        )
        self.assertIn(
            "-n exp-pricing-survey --permission-mode auto -w exp-pricing-survey",
            out,
            result.stderr,
        )
        self.assertIn("MARK=bedrock advisor", out)

    def test_explore_rejects_a_bare_substrate_word(self):
        for verb in (
            "explore anthropic",
            "explore bedrock",
            "explore anthropic anthropic",
        ):
            with self.subTest(verb=verb):
                result, out = self._launch(verb)
                self.assertIn("RC=1", out)
                self.assertIn("Usage: explore [anthropic|bedrock]", result.stderr)

    _TREE = 'mkdir -p "$_DS_REPO/.claude/worktrees/{0}"; '

    def test_a_fresh_explore_inherits_another_verbs_marker(self):
        # `architect 12` on the seat marked the shared tree: a fresh `explore
        # 12` must launch there too, or its reopen would switch provider.
        result, out = self._launch(
            self._TREE.format(12)
            + "_ds_substrate_write 12 anthropic advisor; explore 12; explore 12; "
            + self._MARK.format(12),
            env=self._TOKEN,
        )
        self.assertEqual(out.count("USE=unset SUB=unset"), 2, out + result.stderr)
        self.assertIn("recorded on anthropic", result.stderr)
        self.assertIn("MARK=anthropic advisor", out)
        # A `task` session's bedrock marker: inherited, and left as it was.
        result, out = self._launch(
            self._TREE.format(13)
            + "_ds_substrate_write 13 bedrock; explore 13; "
            + 'print -r -- "TIER=$(<$_DS_SUBSTRATE_DIR/13.tier)"',
            env=self._TOKEN,
        )
        self.assertIn("USE=1 SUB=work-model[1m]", out, result.stderr)
        self.assertIn("TIER=\n", out + "\n")
        # And a word that disagrees with it refuses before launching.
        result, out = self._launch(
            self._TREE.format(13)
            + "_ds_substrate_write 13 bedrock; explore anthropic 13",
            env=self._TOKEN,
        )
        self.assertIn("RC=1", out)
        self.assertNotIn("MODEL=", out)
        self.assertIn("a switch means a fresh worktree", result.stderr)

    def test_a_tree_it_may_not_mark_launches_where_its_reopen_will_read(self):
        # Some other launch made the tree and left no marker; explore may not
        # mark it, and an absent marker reads anthropic, so it launches there.
        result, out = self._launch(
            self._TREE.format(12)
            + 'explore 12; print -r -- "EXISTS=$([[ -f $_DS_SUBSTRATE_DIR/12 ]] && print y || print n)"',
            env=self._TOKEN,
        )
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)
        self.assertIn("EXISTS=n", out)

    def test_a_stale_marker_with_no_tree_or_session_is_rewritten(self):
        # A pruned tree's leftover marker names no live session, so a fresh
        # launch takes its own word and records it.
        result, out = self._launch(
            "_ds_substrate_write exp-survey bedrock advisor; "
            "explore anthropic survey; " + self._MARK.format("exp-survey"),
            env=self._TOKEN,
        )
        self.assertIn("USE=unset SUB=unset", out, result.stderr)
        self.assertIn("MARK=anthropic advisor", out)

    def test_task_resume_hands_an_explore_session_back_to_its_own_verb(self):
        # `fleet` types `task resume <n>`; an explore session comes back through
        # its own verb, which resumes it by its computed id.
        setup = (
            self._TREE.format(13)
            + self._explore_transcript(13)
            + "_ds_substrate_write 13 bedrock advisor; "
        )
        result, out = self._launch(setup + "task resume 13", env=self._TOKEN)
        self.assertIn(
            "USE=1 SUB=work-model[1m] SONNET=help-model[1m] ARGS=--resume",
            out,
            result.stderr,
        )
        # An architect thread on the same number: which session the resume
        # reaches is not knowable here, so the generic path stands.
        architect = (
            "thread_sid=$(_ds_topic_sid architect 13); "
            ': >| "$CLAUDE_CONFIG_DIR/projects/some-slug/$thread_sid.jsonl"; '
        )
        result, out = self._launch(
            setup + architect + "python3() { :; }; task resume 13", env=self._TOKEN
        )
        # The generic path resumes by tag, not id — and pins the sub-agent
        # tiers too, as every Bedrock launch does.
        self.assertIn(
            "USE=1 SUB=work-model[1m] SONNET=help-model[1m] ARGS=--resume 13 ",
            out,
            result.stderr,
        )

    def test_an_unmarked_tree_says_why_it_lands_on_the_seat(self):
        result, out = self._launch(self._TREE.format(12) + "explore 12")
        self.assertIn("no marker", result.stderr)
        self.assertNotIn("recorded on anthropic", result.stderr)

    def test_a_reopen_whose_word_matches_its_record_proceeds(self):
        result, out = self._launch(
            self._explore_transcript(12)
            + "_ds_substrate_write 12 anthropic advisor; explore anthropic 12",
            env=self._TOKEN,
        )
        self.assertIn("RC=0", out, result.stderr)
        self.assertIn("USE=unset", out)
        self.assertIn("ARGS=--resume", out)

    def test_chore_launches_the_helper_tier_on_bedrock_with_no_worktree(self):
        result, out = self._launch("chore eng-7", env=self._TOKEN)
        self.assertIn("MODEL=help-model[1m] USE=1", out, result.stderr)
        # The skill is the whole prompt, and nothing creates a worktree.
        self.assertIn("-n 7 --permission-mode auto /chore 7", out)
        self.assertNotIn("-w", out)
        self.assertNotIn("/init-pr", out)

    #: A first-party-shaped placeholder, so the probe maps it to a profile
    #: rather than skipping it as an alias.
    _PROBED = {"DS_MODEL_HELPER": "claude-help-model[1m]"}

    def _curl_log(self):
        log = self.repo / "curl.log"
        return log.read_text() if log.exists() else ""

    def test_a_bedrock_chore_probes_the_model_before_launching(self):
        result, out = self._launch("chore 7", env={**self._TOKEN, **self._PROBED})
        self.assertIn("MODEL=claude-help-model[1m] USE=1", out, result.stderr)
        log = self._curl_log()
        # The Bedrock profile, window suffix dropped, asked for one token.
        self.assertIn("/model/us.anthropic.claude-help-model/converse", log)
        self.assertIn('"maxTokens":1', log)
        # The key rides stdin, never argv, where `ps` would show it.
        argv, stdin = log.split(" STDIN=", 1)
        self.assertNotIn("placeholder-key", argv)
        self.assertIn("Authorization: Bearer placeholder-key", stdin)

    def test_a_model_that_does_not_answer_launches_nothing(self):
        # The 1843 failure: a tab that 403s on every turn.
        denied = "Model access is denied due to aws-marketplace:Subscribe"
        result, out = self._launch(
            "chore 7 || print -r -- REFUSED; "
            'print -r -- "USE_AFTER=${CLAUDE_CODE_USE_BEDROCK-unset}"',
            env={
                **self._TOKEN,
                **self._PROBED,
                "PROBE_CODE": "403",
                "PROBE_BODY": denied,
            },
        )
        self.assertIn("REFUSED", out)
        self.assertNotIn("MODEL=", out)
        self.assertIn("USE_AFTER=unset", out)
        self.assertIn("HTTP 403", result.stderr)
        self.assertIn(denied, result.stderr)
        # Refused before the marker, so a resume cannot inherit the attempt.
        for name in ("7", "7.tier"):
            self.assertFalse((self.repo / ".claude/session-substrate" / name).exists())

    def test_an_anthropic_chore_and_an_alias_are_not_probed(self):
        result, out = self._launch("chore anthropic 7", env=self._PROBED)
        self.assertIn("MODEL=claude-help-model[1m] USE=unset", out, result.stderr)
        result, out = self._launch("chore 8", env=self._TOKEN)
        self.assertIn("MODEL=help-model[1m] USE=1", out, result.stderr)
        self.assertIn("is an alias, so it cannot be probed", result.stderr)
        self.assertEqual(self._curl_log(), "")

    def test_task_resume_hands_a_chore_back_to_its_own_verb(self):
        # `fleet` types `task resume <n>`; a chore has no worktree to resolve,
        # so it must reopen by the chore's own id, not the picker.
        self._launch("chore anthropic 7")
        result, out = self._launch("task resume 7")
        self.assertIn("/chore 7", out, result.stderr)
        self.assertNotIn("--resume 7", out)
        # On the substrate it recorded, not the helper tier's Bedrock default.
        self.assertIn("MODEL=help-model[1m] USE=unset", out)

    def test_chore_takes_task_s_substrate_words(self):
        result, out = self._launch("chore anthropic 7")
        self.assertIn("MODEL=help-model[1m] USE=unset", out, result.stderr)
        result, out = self._launch(
            "chore bedrock 7",
            env={**self._TOKEN, "DS_MODEL_HELPER_SUBSTRATE": "anthropic"},
        )
        self.assertIn("MODEL=help-model[1m] USE=1", out, result.stderr)

    def test_chore_anthropic_survives_every_resume_path(self):
        self._launch("chore anthropic 7")
        for verb in ("chore 7", "task resume 7"):
            with self.subTest(verb=verb):
                result, out = self._launch(verb, env=self._TOKEN)
                self.assertIn("MODEL=help-model[1m] USE=unset", out, result.stderr)

    def test_chore_leaves_a_task_sessions_marker_alone(self):
        # A mistyped `chore <live-task>` must not re-pin the task's resume
        # onto the helper model.
        marker = 'print -r -- "MARK=$(<$_DS_SUBSTRATE_DIR/12)"'
        result, out = self._launch(
            f"_ds_substrate_write 12 bedrock; chore anthropic 12; {marker}"
        )
        self.assertIn("MARK=bedrock", out, result.stderr)
        self.assertIn("task session's substrate marker", result.stderr)
        result, out = self._launch("task resume 12", env=self._TOKEN)
        self.assertIn("MODEL=work-model[1m] USE=1", out, result.stderr)

    def test_chore_refuses_a_name_zero_and_extra_words(self):
        for verb in ("chore", "chore pricing", "chore 0", "chore 7 8", "chore local 7"):
            with self.subTest(verb=verb):
                result, out = self._launch(verb, env=self._TOKEN)
                self.assertIn("RC=1", out)
                self.assertNotIn("MODEL=", out)

    def test_a_refused_plan_bedrock_login_leaves_no_bedrock_env(self):
        result, out = self._launch(
            "_ds_aws_login() { return 1; }; plan bedrock; "
            'print -r -- "USE=${CLAUDE_CODE_USE_BEDROCK-unset}"',
            env=self._TOKEN,
        )
        self.assertNotIn("MODEL=", out)
        self.assertIn("USE=unset", out)

    def test_architect_takes_the_override_before_the_number(self):
        result, out = self._launch("architect 12")
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)
        # Session, worktree and governing issue all come from the one number.
        self.assertIn("-n 12 --permission-mode auto -w 12 /architect ENG-12", out)
        result, out = self._launch("architect bedrock 12", env=self._TOKEN)
        self.assertIn("MODEL=judge-model[1m] USE=1", out, result.stderr)
        for verb in ("architect local 12", "architect 12 local"):
            with self.subTest(verb=verb):
                result, out = self._launch(verb)
                self.assertIn("RC=1", out)
                self.assertNotIn("MODEL=", out)
        # The retired order launches nothing and names the new one.
        result, out = self._launch("architect 12 bedrock", env=self._TOKEN)
        self.assertIn("RC=1", out)
        self.assertNotIn("MODEL=", out)
        self.assertIn("`architect bedrock 12`", result.stderr)

    def test_architect_bedrock_survives_every_resume_path(self):
        # The third defect: the override wrote no marker, so a resume came back
        # on the anthropic advisor tier.
        self._launch("architect bedrock 12", env=self._TOKEN)
        for verb in ("architect 12", "task resume 12"):
            with self.subTest(verb=verb):
                result, out = self._launch(verb, env=self._TOKEN)
                self.assertIn("MODEL=judge-model[1m] USE=1", out, result.stderr)
        # An explicit word still wins.
        result, out = self._launch("architect anthropic 12")
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)

    def test_architect_leaves_a_task_sessions_marker_alone(self):
        marker = 'print -r -- "MARK=$(<$_DS_SUBSTRATE_DIR/12)"'
        result, out = self._launch(
            f"_ds_substrate_write 12 bedrock; architect anthropic 12; {marker}"
        )
        self.assertIn("MARK=bedrock", out, result.stderr)
        self.assertIn("task session's substrate marker", result.stderr)
        result, out = self._launch("task resume 12", env=self._TOKEN)
        self.assertIn("MODEL=work-model[1m] USE=1", out, result.stderr)

    def test_a_task_launch_takes_the_tier_back(self):
        self._launch("architect 12")
        result, out = self._launch("task 12; task resume 12", env=self._TOKEN)
        self.assertEqual(out.count("MODEL=work-model[1m] USE=1"), 2, out)

    def _transcript(self, slug_of):
        """Write a transcript for `architect 12`'s id under `slug_of`'s slug."""
        return (
            "sid=$(_ds_topic_sid architect 12); "
            f'slug="${{{slug_of}//[\\/.]/-}}"; '
            'mkdir -p "$CLAUDE_CONFIG_DIR/projects/$slug"; '
            ': >| "$CLAUDE_CONFIG_DIR/projects/$slug/$sid.jsonl"; '
            'print -r -- "SID=$sid"; '
        )

    def test_a_reopen_under_the_base_slug_resumes_with_no_prompt(self):
        result, out = self._launch(
            f'cd "$_DS_REPO"; {self._transcript("PWD")}architect 12'
        )
        sid = next(ln[4:] for ln in out.splitlines() if ln.startswith("SID="))
        self.assertIn(f"ARGS=--resume {sid} --permission-mode auto", out)
        self.assertNotIn("/architect", out)
        self.assertNotIn("--session-id", out)

    def test_a_reopen_under_the_worktree_slug_resumes_from_the_worktree(self):
        # The first defect: the probe built only the base-slug path, missed a
        # `-w` session filed under its worktree's slug, and re-ran the
        # bootstrap on top of the existing history.
        wt = self.repo / ".claude" / "worktrees" / "ceo-12"
        wt.mkdir(parents=True)
        result, out = self._launch(
            'claude() { print -r -- "CWD=$PWD ARGS=$*"; }; cd "$_DS_REPO"; '
            'wt="$PWD/.claude/worktrees/ceo-12"; '
            f"{self._transcript('wt')}architect 12"
        )
        sid = next(ln[4:] for ln in out.splitlines() if ln.startswith("SID="))
        self.assertIn(f"/worktrees/ceo-12 ARGS=--resume {sid}", out, result.stderr)
        self.assertNotIn("/architect", out)

    _CWD_STUB = 'claude() { print -r -- "CWD=$PWD ARGS=$*"; }; cd "$_DS_REPO"; '

    def test_a_copy_under_both_slugs_still_resumes_from_the_worktree(self):
        # Glob order is not a contract (here the worktree copy sorts first, as
        # `-` precedes `/`), so this pins the order-independent match: a
        # quoted `(M)` filter over the joined array failed exactly this case.
        (self.repo / ".claude" / "worktrees" / "ceo-12").mkdir(parents=True)
        result, out = self._launch(
            self._CWD_STUB + 'wt="$PWD/.claude/worktrees/ceo-12"; '
            f"{self._transcript('PWD')}{self._transcript('wt')}architect 12"
        )
        self.assertIn("/worktrees/ceo-12 ARGS=--resume", out, result.stderr)

    def test_a_base_slug_copy_beside_a_live_worktree_stays_in_the_base(self):
        (self.repo / ".claude" / "worktrees" / "ceo-12").mkdir(parents=True)
        result, out = self._launch(
            self._CWD_STUB + f"{self._transcript('PWD')}architect 12"
        )
        line = next(ln for ln in out.splitlines() if ln.startswith("CWD="))
        self.assertNotIn("/worktrees/", line, result.stderr)
        self.assertIn("ARGS=--resume", line)

    def test_architect_refuses_a_topic_and_zero(self):
        for verb in ("architect pricing", "architect 0"):
            with self.subTest(verb=verb):
                result, out = self._launch(verb)
                self.assertIn("RC=1", out)
                self.assertNotIn("MODEL=", out)
                self.assertIn("governing issue number", result.stderr)

    def test_a_pre_rename_worktree_is_reused_not_shadowed(self):
        # A bare `<n>` created beside a live `eng-<n>` / `ceo-<n>` would win
        # every later resume and orphan the old thread.
        for name in ("eng-7", "ceo-12"):
            (self.repo / ".claude" / "worktrees" / name).mkdir(parents=True)
        result, out = self._launch("task 7", env=self._TOKEN)
        self.assertIn("-w eng-7 -n 7", out, result.stderr)
        result, out = self._launch("architect 12")
        self.assertIn("-n 12 --permission-mode auto -w ceo-12", out, result.stderr)

    def test_resume_re_pins_a_task_session_on_its_recorded_substrate(self):
        result, out = self._launch(
            "_ds_substrate_write 8 bedrock; task resume 8", env=self._TOKEN
        )
        self.assertIn("MODEL=work-model[1m] USE=1", out, result.stderr)

    def test_resume_reads_a_pre_rename_marker(self):
        # A Bedrock session launched before the rename recorded under `eng-8`;
        # losing it would silently resume that session on the seat.
        result, out = self._launch(
            "_ds_substrate_write eng-8 bedrock; task resume eng-8", env=self._TOKEN
        )
        self.assertIn("MODEL=work-model[1m] USE=1", out, result.stderr)

    def test_resume_of_an_unmarked_tag_re_pins_the_advisor_tier(self):
        # The issue-keyed explore case: no marker, so advisor on anthropic,
        # rather than the saved default the old resume path fell back to.
        result, out = self._launch("task resume 9")
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)

    def test_every_resume_form_keeps_the_permission_mode(self):
        # Omitting it on a resume silently drops to the saved default on every
        # reopen, and `fleet` types `task resume` for every worker session.
        # `python3` is stubbed to stand in for `resolve_session.py`, the only
        # `python3` call on this path, so each of its modes is reached.
        resolved = "python3() { print -r -- %s; print -r -- sid-9; print; }; "
        cases = (
            ("task resume", "ARGS=--resume --permission-mode auto"),
            (
                resolved % "continue" + "task resume 9",
                "ARGS=--continue --permission-mode auto",
            ),
            (
                resolved % "resume" + "task resume 9",
                "ARGS=--resume sid-9 --permission-mode auto",
            ),
            (
                resolved % "none" + "task resume 9",
                "ARGS=--resume 9 --permission-mode auto",
            ),
        )
        for verb, expected in cases:
            with self.subTest(verb=verb):
                result, out = self._launch(verb)
                self.assertIn(expected, out, result.stderr)


class ExitEstimate(SubstrateHarness):
    """The exit epilogue: once `claude` returns, every launcher prints the
    session's estimate and writes its sidecar, without changing the status.

    `python3` is stubbed: `resolve_session.py` answers with `RESOLVED_MODE`
    and `RESOLVED_SID`, and `session_metrics.py` logs its argv and exits with
    `METRICS_RC`.
    """

    _STUBS = VerbLaunch._STUBS
    _AFTER = VerbLaunch._AFTER
    _TOKEN = VerbLaunch._TOKEN
    _launch = VerbLaunch._launch

    _PY = (
        "python3() { case $1 in "
        '*/resolve_session.py) print -r -- x >> "$_DS_REPO/resolve.log"; '
        'print -r -- "${RESOLVED_MODE:-continue}"; '
        'print -r -- "${RESOLVED_SID-sid-9}"; print ;; '
        '*/session_metrics.py) shift; print -r -- "METRICS $*"; '
        "return ${METRICS_RC:-0} ;; "
        "esac; }; "
    )

    def _metrics_lines(self, out):
        return [line for line in out.splitlines() if line.startswith("METRICS ")]

    def test_every_launch_path_prices_the_session_it_recorded(self):
        for verb in ("plan", "architect 12", "explore 12", "chore 12", "task 7"):
            with self.subTest(verb=verb):
                result, out = self._launch(
                    f"rm -rf $_DS_SUBSTRATE_DIR; {self._PY}{verb}; "
                    "print -r -- MARKED $_DS_SUBSTRATE_DIR/sessions/*(N:t)",
                    env=self._TOKEN,
                )
                metrics = self._metrics_lines(out)
                self.assertEqual(len(metrics), 1, result.stderr)
                sid = out.split("MARKED ", 1)[1].split()[0]
                self.assertEqual(
                    metrics[0],
                    f"METRICS --brief --session-id {sid} --estimate-out "
                    f"{self.repo}/.claude/session-substrate/sessions/{sid}.estimate",
                )

    def test_a_failed_estimate_warns_once_and_keeps_the_exit_status(self):
        result, out = self._launch(
            "claude() { return 3; }; " + self._PY + "task 7",
            env={**self._TOKEN, "METRICS_RC": "1"},
        )
        self.assertIn("RC=3", out)
        self.assertEqual(result.stderr.count("no session estimate"), 1, result.stderr)

    def test_a_success_keeps_the_exit_status_too(self):
        result, out = self._launch(
            "claude() { return 4; }; " + self._PY + "plan", env=self._TOKEN
        )
        self.assertIn("RC=4", out, result.stderr)
        self.assertNotIn("no session estimate", result.stderr)

    def test_a_continue_resume_resolves_the_session_by_tag_after_exit(self):
        result, out = self._launch(self._PY + "task resume 9")
        self.assertIn("ARGS=--continue", out, result.stderr)
        metrics = self._metrics_lines(out)
        self.assertEqual(len(metrics), 1, result.stderr)
        self.assertIn("--brief --session-id sid-9 ", metrics[0])
        # Once to launch, once after exit.
        self.assertEqual(self._resolver_calls(), 2)

    def _resolver_calls(self):
        log = self.repo / "resolve.log"
        return len(log.read_text().splitlines()) if log.exists() else 0

    def test_a_resume_by_id_prices_the_id_it_launched_without_resolving_again(self):
        result, out = self._launch(
            self._PY + "task resume 9", env={"RESOLVED_MODE": "resume"}
        )
        self.assertIn("ARGS=--resume sid-9", out, result.stderr)
        self.assertIn("--brief --session-id sid-9 ", self._metrics_lines(out)[0])
        self.assertEqual(self._resolver_calls(), 1)

    def test_an_unresolved_session_warns_and_prices_nothing(self):
        result, out = self._launch(
            self._PY + "task resume 9",
            env={"RESOLVED_MODE": "picker", "RESOLVED_SID": ""},
        )
        self.assertIn("ARGS=--resume 9", out, result.stderr)
        self.assertEqual(self._metrics_lines(out), [])
        self.assertIn("did not resolve", result.stderr)
        self.assertIn("RC=0", out)


class ModelsVerb(SubstrateHarness):
    """`models` — the offline table, and `check` against a stubbed `aws`."""

    #: Logs each `aws` argv to a file (`models` discards the call's own output)
    #: and answers ACTIVE only for `known-*` profiles. `env -u VAR` is stubbed
    #: to drop its two arguments and run the rest.
    _AWS = (
        'aws() { print -r -- "AWS $*" >> "$_DS_REPO/aws.log"; '
        '[[ "$*" == *known-* ]]; }; '
        # Refuses unless the call really strips the bearer token, so dropping
        # `env -u AWS_BEARER_TOKEN_BEDROCK` from `models` fails the suite.
        'env() { [[ "$1 $2" == "-u AWS_BEARER_TOKEN_BEDROCK" ]] || return 9; '
        'shift 2; "$@"; }; '
    )

    def _models(self, args, env=None):
        result = self._zsh(
            self._AWS + f'models {args}; print -r -- "RC=$?"; '
            '[[ -f "$_DS_REPO/aws.log" ]] && print -r -- "$(<$_DS_REPO/aws.log)"',
            env={**_CONFIG, **(env or {})},
        )
        return result, result.stdout

    def test_the_table_is_offline_and_complete(self):
        result, out = self._models("")
        self.assertIn("advisor  judge-model[1m]  (anthropic)", out, result.stderr)
        self.assertIn("executor  work-model[1m]  (bedrock)", out)
        self.assertIn("helper  help-model[1m]  (bedrock)", out)
        self.assertIn("background  bg-profile-id", out)
        self.assertNotIn("AWS ", out)
        self.assertIn("RC=0", out)

    def test_an_unset_tier_makes_the_table_fail(self):
        result, out = self._models("", env={"DS_MODEL_EXECUTOR": ""})
        self.assertIn("advisor  judge-model[1m]", out)
        self.assertNotIn("executor  ", out)
        self.assertIn("RC=1", out)

    def test_check_maps_first_party_ids_in_every_tier(self):
        result, out = self._models(
            "check",
            env={
                "DS_MODEL_ADVISOR": "claude-known-judge[1m]",
                "DS_MODEL_EXECUTOR": "us.anthropic.known-work",
                "DS_MODEL_HELPER": "claude-known-help[1m]",
                "DS_MODEL_BACKGROUND": "claude-known-bg[1m]",
                "DS_AWS_PROFILE": "admin",
            },
        )
        self.assertIn("ok     advisor us.anthropic.claude-known-judge", out)
        self.assertIn("ok     executor us.anthropic.known-work", out)
        self.assertIn("ok     helper us.anthropic.claude-known-help", out)
        # Background is mapped too, the way Claude Code maps it.
        self.assertIn(
            "--inference-profile-identifier us.anthropic.claude-known-bg", out
        )
        self.assertIn("--profile admin", out)
        self.assertIn("RC=0", out)

    def test_check_passes_a_bedrock_form_background_id_through(self):
        result, out = self._models(
            "check", env={"DS_MODEL_BACKGROUND": "us.anthropic.claude-known-bg"}
        )
        self.assertIn("ok     background us.anthropic.claude-known-bg", out)
        self.assertNotIn("us.anthropic.us.anthropic", out)
        self.assertIn("RC=0", out)

    def test_check_fails_a_background_alias(self):
        result, out = self._models(
            "check",
            env={
                "DS_MODEL_ADVISOR": "claude-known-judge[1m]",
                "DS_MODEL_EXECUTOR": "claude-known-work[1m]",
                "DS_MODEL_BACKGROUND": "haiku",
            },
        )
        # Claude Code resolves `haiku` through this very slot, so an alias
        # here is circular and fails rather than being skipped.
        self.assertIn("FAIL   background haiku", out)
        self.assertIn("RC=1", out)

    def test_check_fails_on_an_unknown_profile(self):
        result, out = self._models(
            "check", env={"DS_MODEL_ADVISOR": "claude-missing[1m]"}
        )
        self.assertIn("FAIL   advisor us.anthropic.claude-missing", out)
        # A non-`claude-*`, non-Bedrock id is an alias, reported not failed.
        self.assertIn("skip   executor work-model[1m]", out)
        self.assertIn("RC=1", out)


class TagAndWorktree(SubstrateHarness):
    """`_ds_tag_of` / `_ds_worktree_of` — the bare-number name, and the seam
    that still reaches a worktree created under a retired prefix."""

    def test_every_issue_spelling_is_the_bare_number(self):
        result = self._zsh(
            "for t in 882 eng-882 ENG-882 ceo-882 exp-882 0882 exp-pricing; "
            "do _ds_tag_of $t; done"
        )
        self.assertEqual(
            result.stdout.split(), ["882"] * 6 + ["exp-pricing"], result.stderr
        )

    def _trees(self, *names):
        for name in names:
            (self.repo / ".claude" / "worktrees" / name).mkdir(parents=True)

    def test_the_bare_directory_wins(self):
        self._trees("5", "eng-5")
        result = self._zsh("_ds_worktree_of 5")
        self.assertEqual(result.stdout.strip(), "5")

    def test_a_pre_rename_directory_is_reached(self):
        # The fleet-resume miss: an architect thread lives in `ceo-<n>`.
        self._trees("ceo-6", "eng-7")
        result = self._zsh("_ds_worktree_of 6; _ds_worktree_of 7")
        self.assertEqual(result.stdout.split(), ["ceo-6", "eng-7"])

    def test_no_directory_is_non_zero(self):
        result = self._zsh('_ds_worktree_of 9; print -r -- "rc=$?"')
        self.assertEqual(result.stdout.strip(), "rc=1")


class MarkerRoundTrip(SubstrateHarness):
    """`_ds_substrate_write` / `_ds_substrate_read`."""

    def test_absent_marker_reads_as_anthropic(self):
        # The conservative default, and the one every pre-marker session gets.
        result = self._zsh("_ds_substrate_read eng-999")
        self.assertEqual(result.stdout.strip(), "anthropic")

    def test_the_retired_seat_spelling_reads_as_anthropic(self):
        # Markers written before the vocabulary change must resume unchanged.
        result = self._zsh("_ds_substrate_write eng-5 seat; _ds_substrate_read eng-5")
        self.assertEqual(result.stdout.strip(), "anthropic")

    def test_marker_presence_is_the_resume_tier(self):
        result = self._zsh(
            "_ds_substrate_write eng-6 anthropic; _ds_resume_tier eng-6; "
            "_ds_resume_tier eng-998"
        )
        self.assertEqual(result.stdout.split(), ["executor", "advisor"])

    def test_an_advisor_tier_file_beats_marker_presence(self):
        # The marker stays one bare word for session_metrics.py; the tier rides
        # beside it, and a tier-less rewrite (a `task` launch) drops it again.
        result = self._zsh(
            "_ds_substrate_write 6 bedrock advisor; _ds_resume_tier 6; "
            '_ds_substrate_read 6; print -r -- "$(<$_DS_SUBSTRATE_DIR/6)"; '
            "_ds_substrate_write 6 bedrock; _ds_resume_tier 6"
        )
        self.assertEqual(
            result.stdout.split(), ["advisor", "bedrock", "bedrock", "executor"]
        )

    def test_a_helper_tier_file_is_read_back(self):
        result = self._zsh("_ds_substrate_write 6 bedrock helper; _ds_resume_tier 6")
        self.assertEqual(result.stdout.strip(), "helper")

    def test_bedrock_round_trips(self):
        result = self._zsh(
            "_ds_substrate_write eng-1 bedrock; _ds_substrate_read eng-1"
        )
        self.assertEqual(result.stdout.strip(), "bedrock")

    def test_anthropic_round_trips(self):
        result = self._zsh(
            "_ds_substrate_write eng-2 anthropic; _ds_substrate_read eng-2"
        )
        self.assertEqual(result.stdout.strip(), "anthropic")

    def test_a_garbage_marker_reads_as_anthropic(self):
        # Same reasoning as the absent case: an unparseable value must not be
        # taken as license to spend credits.
        result = self._zsh("_ds_substrate_write eng-3 wat; _ds_substrate_read eng-3")
        self.assertEqual(result.stdout.strip(), "anthropic")

    def test_write_survives_an_unwritable_state_directory(self):
        # Best-effort by design: a marker is an optimization over the seat
        # default, so failing to write one must never fail a launch.
        blocked = self.repo / ".claude" / "session-substrate"
        blocked.write_text("not a directory\n", encoding="utf-8")
        result = self._zsh("_ds_substrate_write eng-4 bedrock; print -r -- rc=$?")
        self.assertIn("rc=0", result.stdout)


class SeatGuard(SubstrateHarness):
    """`_ds_seat_guard` — the seat pin is an absence, so it must be made true."""

    #: Probes EVERY variable `_ds_substrate_unset` touches. Reviewing an
    #: earlier version found it asserted four of five — `FAST` was missing, so
    #: deleting that name from the unset list kept this suite green while a
    #: seat session's background sub-turns went on billing to Bedrock credits.
    #: That is exactly the silent-in-production class the module docstring
    #: names as its selection criterion, so the probe is kept exhaustive.
    _PROBE = (
        'print -r -- "USE=${CLAUDE_CODE_USE_BEDROCK-unset}"; '
        'print -r -- "MODEL=${ANTHROPIC_MODEL-unset}"; '
        'print -r -- "FAST=${ANTHROPIC_DEFAULT_HAIKU_MODEL-unset}"; '
        'print -r -- "REGION=${AWS_REGION-unset}"; '
        'print -r -- "TOKEN=${AWS_BEARER_TOKEN_BEDROCK-unset}"; '
        'print -r -- "CACHE=${ENABLE_PROMPT_CACHING_1H-unset}"'
    )

    def test_the_OWNED_bedrock_exports_are_cleared(self):
        # The concrete slip: run `task 1234`, quit, then `plan` in the same tab.
        # Without this the planning session runs on credits with its Fable pin
        # dropped, and nothing anywhere reports it.
        #
        # Asserts only the four variables this launcher OWNS. `AWS_REGION` and
        # the bearer token are shared with the operator's environment and are
        # restored rather than cleared, which cannot be exercised by hand-set
        # exports like these — there is no recorded launch to undo. Their real
        # behavior is covered end-to-end by `SharedVariableRoundTrip` below.
        result = self._zsh(
            f"_ds_seat_guard plan; {self._PROBE}",
            env={
                "CLAUDE_CODE_USE_BEDROCK": "1",
                "ANTHROPIC_MODEL": "us.anthropic.claude-opus-5[1m]",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "us.anthropic.claude-haiku-x",
                "ENABLE_PROMPT_CACHING_1H": "1",
            },
        )
        self.assertIn("USE=unset", result.stdout)
        self.assertIn("MODEL=unset", result.stdout)
        self.assertIn("FAST=unset", result.stdout)
        self.assertIn("CACHE=unset", result.stdout)

    def test_a_seat_verb_in_a_FRESH_tab_leaves_shared_variables_alone(self):
        # `AWS_REGION` and the token are shared with the operator's own
        # environment. With no launch recorded in this shell they were never
        # ours, so a plain `plan` in a fresh tab must not destroy an
        # `AWS_REGION` the shell profile exported.
        result = self._zsh(
            f"_ds_seat_guard plan; {self._PROBE}",
            env={
                "AWS_REGION": "eu-central-1",
                "AWS_BEARER_TOKEN_BEDROCK": "operator-supplied",
            },
        )
        self.assertIn("REGION=eu-central-1", result.stdout)
        self.assertIn("TOKEN=operator-supplied", result.stdout)


@unittest.skipUnless(shutil.which("zsh"), _NEEDS_ZSH)
class SharedVariableRoundTrip(SubstrateHarness):
    """End-to-end: drive `_ds_bedrock_env` for real, then run the seat guard.

    These exist because the first attempt at provenance tracking was tested by
    hand-setting its marker, which is exactly where its two bugs lived: the
    marker went stale on a SECOND launch in one tab (so a launcher-resolved
    token then survived a seat verb), and a hand-swapped token was destroyed
    anyway. Neither is reachable from a test that sets the marker itself.
    """

    _PROBE = SeatGuard._PROBE

    #: Enough for `_ds_bedrock_env` to succeed without touching 1Password.
    _TOKEN_ENV = {"AWS_BEARER_TOKEN_BEDROCK": "operator-supplied"}

    def test_an_operator_token_and_region_survive_a_real_launch_and_guard(self):
        result = self._zsh(
            f"_ds_bedrock_env; _ds_seat_guard plan; {self._PROBE}",
            env={**self._TOKEN_ENV, "AWS_REGION": "eu-central-1"},
        )
        self.assertIn("USE=unset", result.stdout)
        self.assertIn("REGION=eu-central-1", result.stdout)
        self.assertIn("TOKEN=operator-supplied", result.stdout)

    def test_a_region_the_launcher_installed_is_undone(self):
        # No operator region beforehand, so the restore is to "absent".
        result = self._zsh(
            f"_ds_bedrock_env; _ds_seat_guard plan; {self._PROBE}",
            env=dict(self._TOKEN_ENV),
        )
        self.assertIn("REGION=unset", result.stdout)

    def test_a_FAILED_launch_does_not_destroy_the_operators_region(self):
        # `_ds_bedrock_env` exports AWS_REGION before it can discover it has no
        # token, and its failure path rolls back through the same function the
        # seat guard uses. The rollback must undo the launch, not clear.
        result = self._zsh(
            f"_ds_bedrock_env; {self._PROBE}",
            env={"AWS_REGION": "eu-central-1"},
        )
        self.assertIn("no Bedrock bearer token", result.stderr)
        self.assertIn("REGION=eu-central-1", result.stdout)

    def test_a_SECOND_launch_in_one_tab_still_restores_the_operators_values(self):
        # The staleness bug: recording again on the second launch would capture
        # the FIRST launch's values as "prior", so the restore would put
        # Bedrock's region back instead of the operator's.
        result = self._zsh(
            f"_ds_bedrock_env; _ds_bedrock_env; _ds_seat_guard plan; {self._PROBE}",
            env={**self._TOKEN_ENV, "AWS_REGION": "eu-central-1"},
        )
        self.assertIn("REGION=eu-central-1", result.stdout)
        self.assertIn("TOKEN=operator-supplied", result.stdout)

    def test_a_token_swapped_BY_HAND_after_a_launch_is_left_alone(self):
        # It no longer matches what the launch installed, so it is the
        # operator's and stays — the vice-versa of the case above.
        result = self._zsh(
            "_ds_bedrock_env; export AWS_BEARER_TOKEN_BEDROCK=swapped-by-hand; "
            f"_ds_seat_guard plan; {self._PROBE}",
            env=dict(self._TOKEN_ENV),
        )
        self.assertIn("TOKEN=swapped-by-hand", result.stdout)


@unittest.skipUnless(shutil.which("zsh"), _NEEDS_ZSH)
class LauncherResolvedToken(SubstrateHarness):
    """The 1Password path — the one an actual operator launch takes.

    Every other case here supplies `AWS_BEARER_TOKEN_BEDROCK` directly, which
    exercises the `${VAR:-…}` escape hatch and skips the resolution entirely.
    A stub `op` on PATH reaches the real branch at no extra cost, and it is the
    only way to test the direction that matters most: a token the LAUNCHER
    produced must not outlive a seat verb.
    """

    _PROBE = SeatGuard._PROBE
    _RESOLVED = "stub-resolved-token"

    def _op_env(self):
        bin_dir = Path(self._tmp.name) / "bin"
        bin_dir.mkdir(exist_ok=True)
        stub = bin_dir / "op"
        stub.write_text(f"#!/bin/sh\necho {self._RESOLVED}\n", encoding="utf-8")
        stub.chmod(0o755)
        return {
            "PATH": f"{bin_dir}:{os.environ.get('PATH', '')}",
            "DS_OP_ACCOUNT": "example.1password.com",
            "DS_OP_BEDROCK_REF": "op://vault/item/credential",
        }

    def test_the_token_is_resolved_from_the_configured_reference(self):
        result = self._zsh(
            f'_ds_bedrock_env; print -r -- "rc=$?"; {self._PROBE}',
            env=self._op_env(),
        )
        self.assertIn("rc=0", result.stdout)
        self.assertIn(f"TOKEN={self._RESOLVED}", result.stdout)

    def test_a_launcher_resolved_token_does_NOT_outlive_a_seat_verb(self):
        result = self._zsh(
            f"_ds_bedrock_env; _ds_seat_guard plan; {self._PROBE}",
            env=self._op_env(),
        )
        self.assertIn("USE=unset", result.stdout)
        self.assertIn("TOKEN=unset", result.stdout)

    def test_it_still_does_not_outlive_a_SECOND_launch_in_the_same_tab(self):
        # The staleness bug in the first provenance attempt: the second launch
        # saw a token already present, concluded it was the operator's, and the
        # seat guard then preserved a launcher-resolved credential in a seat
        # tab. Two launches, one guard, token must still be gone.
        result = self._zsh(
            f"_ds_bedrock_env; _ds_bedrock_env; _ds_seat_guard plan; {self._PROBE}",
            env=self._op_env(),
        )
        self.assertIn("TOKEN=unset", result.stdout)

    def test_clearing_is_announced(self):
        # The warning is kept alongside the correction: a silent fix hides that
        # the tab was in an unexpected state to begin with.
        result = self._zsh("_ds_seat_guard plan", env={"CLAUDE_CODE_USE_BEDROCK": "1"})
        self.assertIn("plan:", result.stderr)
        self.assertIn("seat verb", result.stderr)

    def test_a_clean_shell_is_silent(self):
        result = self._zsh(f"_ds_seat_guard plan; {self._PROBE}")
        self.assertEqual(result.stderr.strip(), "")
        self.assertIn("USE=unset", result.stdout)


class BedrockEnvGate(SubstrateHarness):
    """`_ds_bedrock_env` — non-zero means do not launch."""

    def test_no_token_is_a_hard_failure(self):
        # The gate exists because launching without a token produces an opaque
        # provider error several turns in, long after work has started.
        result = self._zsh("_ds_bedrock_env")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no Bedrock bearer token", result.stderr)

    def test_a_failed_gate_leaves_no_half_set_environment(self):
        # A launch that is refused must not leave the tab pinned to Bedrock —
        # that would make the NEXT seat verb in the same tab wrong too.
        result = self._zsh(
            '_ds_bedrock_env; print -r -- "USE=${CLAUDE_CODE_USE_BEDROCK-unset}"'
        )
        self.assertIn("USE=unset", result.stdout)

    def test_an_exported_token_satisfies_the_gate(self):
        # The `${VAR:-…}` override path: a key pinned by hand wins, which is
        # also the escape hatch when no 1Password coordinates are configured.
        result = self._zsh(
            '_ds_bedrock_env; print -r -- "rc=$?"; '
            'print -r -- "MODEL=$ANTHROPIC_MODEL"; '
            'print -r -- "REGION=$AWS_REGION"; '
            'print -r -- "FAST=$ANTHROPIC_DEFAULT_HAIKU_MODEL"; '
            'print -r -- "CACHE=$ENABLE_PROMPT_CACHING_1H"',
            env={**_CONFIG, "AWS_BEARER_TOKEN_BEDROCK": "placeholder-key"},
        )
        self.assertIn("rc=0", result.stdout)
        # The model is NOT exported: the verb scopes it to its one `claude`
        # command, so it cannot linger in the tab.
        self.assertIn("MODEL=\n", result.stdout)
        self.assertIn("REGION=us-west-2", result.stdout)
        self.assertIn("CACHE=1", result.stdout)
        # No sub-agent slot is exported into the tab either: `_ds_claude` binds
        # them per command (asserted in `test_shell_launcher.py`).
        self.assertIn("FAST=\n", result.stdout)

    def test_an_unset_background_tier_warns_and_pins_nothing(self):
        # Background failures degrade niceties only, so this warns rather than
        # refusing — and must not leave a stale pin from an earlier launch.
        result = self._zsh(
            '_ds_bedrock_env; print -r -- "rc=$?"; '
            'print -r -- "FAST=${ANTHROPIC_DEFAULT_HAIKU_MODEL-unset}"',
            env={
                "AWS_BEARER_TOKEN_BEDROCK": "placeholder-key",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "stale-id",
            },
        )
        self.assertIn("rc=0", result.stdout)
        self.assertIn("FAST=unset", result.stdout)
        self.assertIn("DS_MODEL_BACKGROUND is unset", result.stderr)

    def test_the_opus_alias_is_the_executor_and_a_seat_verb_clears_it(self):
        # The `plan` skill's lever fold names `model: opus`; on Bedrock that
        # must be the executor, not the advisor the session itself runs.
        # The slot is bound for the one `claude` command, not the tab.
        probe = 'print -r -- "OPUS=${ANTHROPIC_DEFAULT_OPUS_MODEL-unset}"'
        stub = (
            'claude() { print -r -- "IN=$(printenv ANTHROPIC_DEFAULT_OPUS_MODEL)"; }; '
        )
        result = self._zsh(
            f"{stub}_ds_bedrock_env; _ds_claude m; {probe}; _ds_seat_guard plan; "
            f"{probe}",
            env={**_CONFIG, "AWS_BEARER_TOKEN_BEDROCK": "placeholder-key"},
        )
        self.assertIn("IN=work-model[1m]\nOPUS=unset\nOPUS=unset\n", result.stdout)
        # Unset, no stale pin from an earlier launch survives.
        result = self._zsh(
            f"_ds_bedrock_env; {probe}",
            env={
                "AWS_BEARER_TOKEN_BEDROCK": "placeholder-key",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "stale-id",
            },
        )
        self.assertIn("OPUS=unset", result.stdout)

    def test_the_region_is_overridable(self):
        result = self._zsh(
            '_ds_bedrock_env; print -r -- "REGION=$AWS_REGION"',
            env={
                "AWS_BEARER_TOKEN_BEDROCK": "placeholder-key",
                "DS_BEDROCK_REGION": "us-east-1",
            },
        )
        self.assertIn("REGION=us-east-1", result.stdout)


@unittest.skipUnless(shutil.which("zsh"), _NEEDS_ZSH)
class VerbSurface(unittest.TestCase):
    """The retired verbs are gone and the new ones exist — a clean cut.

    Ratified as "no aliases": the family is small and every launcher is the
    operator's own muscle memory, so a half-migration that leaves both names
    working is the outcome to avoid.
    """

    def _defined(self, name):
        result = subprocess.run(
            ["zsh", "-c", f'source "{INIT}" 2>/dev/null; whence -w {name}'],
            capture_output=True,
            text=True,
            check=False,
        )
        return f"{name}: function" in result.stdout

    def test_new_verbs_are_defined(self):
        for verb in (
            "task",
            "explore",
            "chore",
            "plan",
            "architect",
            "fleet",
            "cdds",
            "models",
        ):
            with self.subTest(verb=verb):
                self.assertTrue(self._defined(verb), f"{verb} is not defined")

    def test_retired_verbs_are_gone(self):
        # `housekeeping` too: deleted, not aliased, once `plan` absorbed it.
        retired = ("aps", "raps", "naps", "rnaps", "paps", "haps", "caps", "faps")
        for verb in (*retired, "housekeeping"):
            with self.subTest(verb=verb):
                self.assertFalse(
                    self._defined(verb), f"{verb} still defined; the cut is not clean"
                )


if __name__ == "__main__":
    unittest.main()
