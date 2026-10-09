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

# The helpers are zsh and deliberately so (`<->` all-digit tests, `>|` clobber),
# and the Linux CI runner ships no zsh: without this guard every case fails with
# FileNotFoundError, which says nothing about the code under test.
_NEEDS_ZSH = "the session helpers are zsh; no zsh on this machine"

#: Cleared from the inherited environment so a case tests the helper rather than
#: the operator's own shell profile — several of these are exactly what an
#: operator running Bedrock sessions has exported.
_SUITE_OWNED_ENV = (
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_MODEL",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_REGION",
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

    def test_an_unset_model_refuses_and_names_the_variable(self):
        # No committed fallback: an unset tier must not launch on some id the
        # repo carried, which is the generation-stale slip the tiers retire.
        for tier, var in (
            ("advisor", "DS_MODEL_ADVISOR"),
            ("executor", "DS_MODEL_EXECUTOR"),
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
        # `plan bedrock` in a credit pinch: one word, no config edit.
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
        ' USE=${CLAUDE_CODE_USE_BEDROCK-unset} ARGS=$*"; }; '
        "_ds_base() { :; }; _ds_secrets() { :; }; _ds_aws_login() { :; }; "
        "_ds_pull() { :; }; "
    )
    _AFTER = '; print -r -- "RC=$?"; print -r -- "AFTER=${ANTHROPIC_MODEL-unset}"'
    _TOKEN = {"AWS_BEARER_TOKEN_BEDROCK": "placeholder-key"}

    def _launch(self, verb, env=None):
        result = self._zsh(
            self._STUBS + verb + self._AFTER, env={**_CONFIG, **(env or {})}
        )
        return result, result.stdout

    def test_plan_launches_the_advisor_tier_on_anthropic(self):
        result, out = self._launch("plan")
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)
        # No `--model` flag: the environment is the one delivery mechanism.
        self.assertNotIn("--model", out)
        self.assertIn("AFTER=unset", out)

    def test_plan_bedrock_is_the_pinch_override(self):
        result, out = self._launch("plan bedrock", env=self._TOKEN)
        self.assertIn("MODEL=judge-model[1m] USE=1", out, result.stderr)

    def test_plan_rejects_any_other_word(self):
        result, out = self._launch("plan local")
        self.assertIn("RC=1", out)
        self.assertNotIn("MODEL=", out)

    def test_a_plan_after_a_task_in_the_same_tab_is_not_on_bedrock(self):
        # The inherited Bedrock pin must lose to the verb's own tier.
        result, out = self._launch("task 7; plan", env=self._TOKEN)
        lines = [ln for ln in out.splitlines() if ln.startswith("MODEL=")]
        self.assertEqual(len(lines), 2, out)
        self.assertTrue(lines[0].startswith("MODEL=work-model[1m] USE=1"))
        self.assertTrue(lines[1].startswith("MODEL=judge-model[1m] USE=unset"))

    def test_task_launches_the_executor_tier_on_bedrock(self):
        result, out = self._launch("task 7", env=self._TOKEN)
        self.assertIn("MODEL=work-model[1m] USE=1", out, result.stderr)
        self.assertIn("-w eng-7", out)

    def test_task_anthropic_and_the_retired_local_alias(self):
        # The marker the launch writes is read raw: `_ds_substrate_read` would
        # map a stale `seat` to anthropic too, so it cannot tell them apart.
        marker = 'print -r -- "MARK=$(<$_DS_SUBSTRATE_DIR/eng-7)"'
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

    def test_explore_is_pinned_to_anthropic_whatever_the_advisor_config(self):
        # It writes no marker, so a resume always lands on anthropic; launching
        # it anywhere else would make that resume switch provider.
        result, out = self._launch(
            "explore 12",
            env={**self._TOKEN, "DS_MODEL_ADVISOR_SUBSTRATE": "bedrock"},
        )
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)
        self.assertIn("-w eng-12", out)

    def test_a_refused_plan_bedrock_login_leaves_no_bedrock_env(self):
        result, out = self._launch(
            "_ds_aws_login() { return 1; }; plan bedrock; "
            'print -r -- "USE=${CLAUDE_CODE_USE_BEDROCK-unset}"',
            env=self._TOKEN,
        )
        self.assertNotIn("MODEL=", out)
        self.assertIn("USE=unset", out)

    def test_housekeeping_runs_the_executor_model_on_anthropic_always(self):
        result, out = self._launch(
            "housekeeping", env={"DS_MODEL_EXECUTOR_SUBSTRATE": "bedrock"}
        )
        self.assertIn("MODEL=work-model[1m] USE=unset", out, result.stderr)

    def test_architect_takes_the_override_after_the_topic(self):
        result, out = self._launch("architect pricing bedrock", env=self._TOKEN)
        self.assertIn("MODEL=judge-model[1m] USE=1", out, result.stderr)
        result, out = self._launch("architect pricing")
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)
        result, out = self._launch("architect pricing local")
        self.assertIn("RC=1", out)
        self.assertNotIn("MODEL=", out)

    def test_resume_re_pins_a_task_session_on_its_recorded_substrate(self):
        result, out = self._launch(
            "_ds_substrate_write eng-8 bedrock; task resume 8", env=self._TOKEN
        )
        self.assertIn("MODEL=work-model[1m] USE=1", out, result.stderr)

    def test_resume_of_an_unmarked_tag_re_pins_the_advisor_tier(self):
        # The issue-keyed explore case: no marker, so advisor on anthropic,
        # rather than the saved default the old resume path fell back to.
        result, out = self._launch("task resume 9")
        self.assertIn("MODEL=judge-model[1m] USE=unset", out, result.stderr)


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
                "DS_MODEL_BACKGROUND": "claude-known-bg",
                "DS_AWS_PROFILE": "admin",
            },
        )
        self.assertIn("ok     advisor us.anthropic.claude-known-judge", out)
        self.assertIn("ok     executor us.anthropic.known-work", out)
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

    def test_check_skips_a_background_alias(self):
        result, out = self._models(
            "check",
            env={
                "DS_MODEL_ADVISOR": "claude-known-judge[1m]",
                "DS_MODEL_EXECUTOR": "claude-known-work[1m]",
                "DS_MODEL_BACKGROUND": "haiku",
            },
        )
        self.assertIn("skip   background haiku", out)
        self.assertIn("RC=0", out)

    def test_check_fails_on_an_unknown_profile(self):
        result, out = self._models(
            "check", env={"DS_MODEL_ADVISOR": "claude-missing[1m]"}
        )
        self.assertIn("FAIL   advisor us.anthropic.claude-missing", out)
        # A non-`claude-*`, non-Bedrock id is an alias, reported not failed.
        self.assertIn("skip   executor work-model[1m]", out)
        self.assertIn("RC=1", out)


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
        # The background tier is pinned so background sub-turns bill to credits
        # too, rather than quietly falling back to the subscription. Exported
        # as configured and newline-anchored: the mapping is Claude Code's.
        self.assertIn("FAST=bg-profile-id\n", result.stdout)

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
            "plan",
            "housekeeping",
            "architect",
            "fleet",
            "cdds",
            "models",
        ):
            with self.subTest(verb=verb):
                self.assertTrue(self._defined(verb), f"{verb} is not defined")

    def test_retired_verbs_are_gone(self):
        for verb in ("aps", "raps", "naps", "rnaps", "paps", "haps", "caps", "faps"):
            with self.subTest(verb=verb):
                self.assertFalse(
                    self._defined(verb), f"{verb} still defined; the cut is not clean"
                )


if __name__ == "__main__":
    unittest.main()
