#!/usr/bin/env python3
"""Behavior tests for the session verbs' SUBSTRATE machinery (stdlib unittest).

Three ratified properties are asserted here, each of which fails silently in
production if it regresses — which is the whole reason they are tested rather
than reviewed:

  * The `[1m]` suffix survives the fallback composition. Bedrock defaults an
    model with no suffix to the 200k window and reports nothing, so the symptom of
    losing it is a session with four fifths of its context gone that looks
    exactly like a session that filled up.
  * An absent marker reads as `seat`. Every session predating markers is a
    seat session, and the conservative error is spending the subscription
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
    "DS_BEDROCK_MODEL",
    "DS_BEDROCK_PROBE",
    "DS_BEDROCK_REGION",
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


class ModelComposition(SubstrateHarness):
    """`_ds_bedrock_model` — the `[1m]` suffix is the point."""

    def test_fallback_composition_ends_in_the_1m_suffix(self):
        # The ratified assertion, stated exactly as the spec states it: the
        # stack exports a BARE profile id, and the launcher is what appends the
        # window. If this ever regresses, nothing at runtime says so.
        result = self._zsh("_ds_bedrock_model")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(
            result.stdout.strip().endswith("[1m]"),
            f"fallback model lost its window suffix: {result.stdout!r}",
        )

    def test_fallback_is_the_us_cross_region_profile(self):
        # `us.` rather than `global.`: the global profile routes to a
        # region-less ARN that no IAM policy can pin, so residency could only be
        # asserted. See infra/aws/README.md.
        result = self._zsh("_ds_bedrock_model")
        self.assertTrue(
            result.stdout.strip().startswith("us."),
            f"fallback model is not a us. profile: {result.stdout!r}",
        )

    def test_configured_model_is_used_verbatim(self):
        # The override exists so a model or window change is a one-line personal
        # config edit. Rewriting it here would defeat that.
        result = self._zsh(
            "_ds_bedrock_model",
            env={"DS_BEDROCK_MODEL": "us.anthropic.claude-fable-5-1[1m]"},
        )
        self.assertEqual(result.stdout.strip(), "us.anthropic.claude-fable-5-1[1m]")

    def test_configured_model_without_a_suffix_warns_but_is_honored(self):
        # Warn, never refuse: the override is the operator's to make, and a
        # refusal would make the escape hatch unusable for the deliberate case.
        result = self._zsh(
            "_ds_bedrock_model",
            env={"DS_BEDROCK_MODEL": "us.anthropic.claude-opus-5"},
        )
        self.assertEqual(result.stdout.strip(), "us.anthropic.claude-opus-5")
        self.assertIn("no context-window", result.stderr)

    def test_a_suffixed_configured_model_is_silent(self):
        result = self._zsh(
            "_ds_bedrock_model",
            env={"DS_BEDROCK_MODEL": "us.anthropic.claude-opus-5[200k]"},
        )
        # The positive assertion is not decoration. On its own the
        # `assertNotIn` below passes vacuously — if the function emitted
        # nothing, exited non-zero, or did not exist at all, stderr is still
        # silent and the case still goes green.
        self.assertEqual(result.stdout.strip(), "us.anthropic.claude-opus-5[200k]")
        self.assertNotIn("no context-window", result.stderr)


class MarkerRoundTrip(SubstrateHarness):
    """`_ds_substrate_write` / `_ds_substrate_read`."""

    def test_absent_marker_reads_as_seat(self):
        # The conservative default, and the one every pre-marker session gets.
        result = self._zsh("_ds_substrate_read eng-999")
        self.assertEqual(result.stdout.strip(), "seat")

    def test_bedrock_round_trips(self):
        result = self._zsh(
            "_ds_substrate_write eng-1 bedrock; _ds_substrate_read eng-1"
        )
        self.assertEqual(result.stdout.strip(), "bedrock")

    def test_seat_round_trips(self):
        result = self._zsh("_ds_substrate_write eng-2 seat; _ds_substrate_read eng-2")
        self.assertEqual(result.stdout.strip(), "seat")

    def test_a_garbage_marker_reads_as_seat(self):
        # Same reasoning as the absent case: an unparseable value must not be
        # taken as license to spend credits.
        result = self._zsh("_ds_substrate_write eng-3 wat; _ds_substrate_read eng-3")
        self.assertEqual(result.stdout.strip(), "seat")

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

    def test_inherited_bedrock_exports_are_cleared(self):
        # The concrete slip: run `task 1234`, quit, then `plan` in the same tab.
        # Without this the planning session runs on credits with its Fable pin
        # dropped, and nothing anywhere reports it.
        result = self._zsh(
            f"_ds_seat_guard plan; {self._PROBE}",
            env={
                "CLAUDE_CODE_USE_BEDROCK": "1",
                "ANTHROPIC_MODEL": "us.anthropic.claude-opus-5[1m]",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "us.anthropic.claude-haiku-x",
                "AWS_REGION": "us-west-2",
                "ENABLE_PROMPT_CACHING_1H": "1",
            },
        )
        self.assertIn("USE=unset", result.stdout)
        self.assertIn("MODEL=unset", result.stdout)
        self.assertIn("FAST=unset", result.stdout)
        self.assertIn("REGION=unset", result.stdout)
        self.assertIn("CACHE=unset", result.stdout)

    def test_an_operator_supplied_token_SURVIVES_the_seat_guard(self):
        # The token is the one deliberate exception, and its direction matters.
        # `_ds_bedrock_env`'s `${VAR:-…}` form exists so an operator can export
        # their own key and run with no 1Password coordinates at all.
        # Destroying it here would make the NEXT `task` in the same tab fail,
        # pointing at config they deliberately did not set.
        result = self._zsh(
            f"_ds_seat_guard plan; {self._PROBE}",
            env={
                "CLAUDE_CODE_USE_BEDROCK": "1",
                "AWS_BEARER_TOKEN_BEDROCK": "operator-supplied",
            },
        )
        self.assertIn("USE=unset", result.stdout)
        self.assertIn("TOKEN=operator-supplied", result.stdout)

    def test_a_launcher_resolved_token_IS_cleared(self):
        # The other side of the same rule: what the launcher itself resolved is
        # the launcher's to clean up. `_DS_TOKEN_FROM_LAUNCHER` is the marker
        # `_ds_bedrock_env` sets when it, rather than the operator, produced the
        # value.
        result = self._zsh(
            f"_DS_TOKEN_FROM_LAUNCHER=1; _ds_seat_guard plan; {self._PROBE}",
            env={
                "CLAUDE_CODE_USE_BEDROCK": "1",
                "AWS_BEARER_TOKEN_BEDROCK": "launcher-resolved",
            },
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
            env={"AWS_BEARER_TOKEN_BEDROCK": "placeholder-key"},
        )
        self.assertIn("rc=0", result.stdout)
        self.assertIn("MODEL=us.anthropic.claude-opus-5[1m]", result.stdout)
        self.assertIn("REGION=us-west-2", result.stdout)
        self.assertIn("CACHE=1", result.stdout)
        # The fast tier is pinned so background sub-turns bill to credits too,
        # rather than quietly falling back to the subscription.
        self.assertIn("FAST=us.anthropic.claude-haiku", result.stdout)

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
