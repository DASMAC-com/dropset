#!/usr/bin/env python3
# cspell:word testverb
"""`_ds_aws_login` — the AWS login gate the seat verbs launch behind.

Every case stubs `aws` as a shell function, so no case can trigger a real
interactive browser login. That is the whole reason this suite exists in this
shape: the behavior under test is *when* the helper decides to log in and *when
it refuses to launch*, and both are unobservable if the real CLI is in the loop.

The load-bearing assertion is `PostLoginProbeDecides` — `aws login` can exit 0
having left a profile that still cannot call STS, so the gate re-probes and
trusts only that. The invariant defended is "this session's credentials
RESOLVE", never "a login ran" — and deliberately not "this session can read cost
data", which `sts:GetCallerIdentity` cannot establish, being authorization-free.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

INIT = Path(__file__).resolve().parents[1] / "shell" / "init.zsh"

# Resolved once, ABSOLUTELY, because the absent-CLI cases empty `PATH` to hide
# `aws` — and a bare "zsh" in the argv would then not be found either, failing
# those cases for a reason that has nothing to do with the helper.
ZSH = shutil.which("zsh")

# Same guard and reason as the sibling shell suites: the helpers are zsh and the
# Linux CI runner ships none, so without this every case fails with
# FileNotFoundError, which says nothing about the code under test.
_NEEDS_ZSH = "the session helpers are zsh; no zsh on this machine"

#: Cleared from the inherited environment so a case tests the helper rather than
#: the operator's own shell profile. `DS_AWS_PROFILE` and the `AWS_*` trio are
#: exactly what an operator with a live SSO session has exported, and
#: `AWS_REGION` in particular changes what the real CLI would do.
_SUITE_OWNED_ENV = (
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_PROFILE",
    "AWS_REGION",
    "CLAUDE_CODE_USE_BEDROCK",
    "DS_AWS_PROFILE",
    "_DS_REPO",
)

# A stub `aws` that records every invocation and dispatches on the subcommand.
# `sts` returns whatever `STS_RC` currently holds, so a case can make the probe
# fail and then have `login` repair it — which is the ordinary expired-session
# path. Defining it as a shell FUNCTION is what keeps the real CLI out of reach:
# `command -v aws` still finds it, so the presence check passes.
# Note both stub variables are EXIT CODES, not booleans — `0` is success, so
# `POST_LOGIN_STS_RC=0` is the repaired case. Spelling them as return codes
# rather than as `LOGIN_REPAIRS`-style flags keeps the polarity readable at the
# call site, where an inverted-looking `0` is otherwise easy to misread.
#
# It records `$#` and the arguments joined on `|` — NOT `"$*"`. That is the whole
# fidelity of this suite's profile assertions, and `"$*"` silently destroyed it:
# space-joining argv makes a one-word `--profile ds-dev` indistinguishable from
# two proper words, and makes a stray empty argument invisible entirely. Both are
# real bugs (`"${profile[*]}"` in place of `"${profile[@]}"` produces exactly
# them), and both passed the space-joined assertions. Verified by mutation: with
# `[*]` substituted, all 15 cases stayed green. The arity plus a delimiter that
# cannot occur in an AWS argument is what makes the boundary observable.
#
# A `default)` branch returns 42 so an unexpected subcommand fails loudly rather
# than reading as a success.
# `login help` is the subcommand-EXISTS probe and must be distinguished from a
# real `login`, or the probe itself would trip the repair side-effect below and
# every expired case would silently self-heal before logging in.
_STUB = (
    'aws() { print -r -- "$#|${(j:|:)@}" >> "$CALLS"; '
    '[[ "$1" == login && "$2" == help ]] && return $LOGIN_HELP_RC; '
    'case "$1" in '
    "sts) return $STS_RC ;; "
    "login) STS_RC=$POST_LOGIN_STS_RC; return $LOGIN_RC ;; "
    "*) return 42 ;; "
    "esac }; "
)


@unittest.skipUnless(shutil.which("zsh"), _NEEDS_ZSH)
class GateHarness(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.calls = Path(self._tmp.name) / "calls"
        self.calls.touch()

    def _gate(
        self,
        *,
        sts_rc=0,
        login_rc=0,
        post_login_sts_rc=0,
        login_help_rc=0,
        env=None,
        stub=True,
    ):
        """Run `_ds_aws_login` against the stub and return (rc, recorded calls)."""
        body = _STUB if stub else ""
        script = 'source "%s" 2>/dev/null; %s_ds_aws_login testverb' % (INIT, body)
        child_env = {**os.environ}
        for name in _SUITE_OWNED_ENV:
            child_env.pop(name, None)
        child_env.update(
            {
                "CALLS": str(self.calls),
                "STS_RC": str(sts_rc),
                "LOGIN_RC": str(login_rc),
                "POST_LOGIN_STS_RC": str(post_login_sts_rc),
                "LOGIN_HELP_RC": str(login_help_rc),
            }
        )
        child_env.update(env or {})
        # `ZSH` with no `or "zsh"` fallback: the class-level `skipUnless` already
        # guarantees it resolved, so a fallback here is unreachable — and it would
        # reintroduce the bare-`zsh`-on-emptied-`PATH` failure the constant exists
        # to prevent, in exactly the cases that empty `PATH`.
        result = subprocess.run(
            [ZSH, "-c", script],
            capture_output=True,
            text=True,
            check=False,
            env=child_env,
        )
        # Each recorded line is `argc|arg1|arg2|…`, so a case can assert on real
        # argument BOUNDARIES rather than on a space-joined string.
        recorded = [
            line.split("|") for line in self.calls.read_text("utf-8").splitlines()
        ]
        return result, [(int(row[0]), row[1:]) for row in recorded]


class ValidSessionCostsNothing(GateHarness):
    """A live session must not pay for a browser."""

    def test_valid_credentials_allow_the_launch(self):
        result, _ = self._gate(sts_rc=0)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_valid_credentials_never_run_a_login(self):
        # The probe-before-login shape is the point: `aws login` opens a browser,
        # so a still-valid session that re-logs in has made the gate a tax on
        # every launch rather than a guard on a broken one.
        _, recorded = self._gate(sts_rc=0)
        self.assertEqual(len(recorded), 1, recorded)
        self.assertEqual(recorded[0][1][0], "sts", recorded)


class ExpiredSessionLogsInOnce(GateHarness):
    """The measured failure: an expired token at launch."""

    def test_expired_then_repaired_allows_the_launch(self):
        result, _ = self._gate(sts_rc=1, login_rc=0, post_login_sts_rc=0)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_expired_then_repaired_logs_in_exactly_once(self):
        _, recorded = self._gate(sts_rc=1, login_rc=0, post_login_sts_rc=0)
        # `help not in args` excludes the subcommand-exists probe, which is also
        # spelled `login`. Counting it would read as two logins.
        logins = [
            args for _, args in recorded if args[0] == "login" and "help" not in args
        ]
        self.assertEqual(len(logins), 1, recorded)

    def test_the_probe_runs_again_after_the_login(self):
        # Two `sts` calls, not one: the second is what actually clears the gate.
        _, recorded = self._gate(sts_rc=1, login_rc=0, post_login_sts_rc=0)
        probes = [args for _, args in recorded if args[0] == "sts"]
        self.assertEqual(len(probes), 2, recorded)


class PostLoginProbeDecides(GateHarness):
    """THE assertion. A login that exits 0 without usable credentials must still
    refuse the launch, because the session it would start cannot read cost data
    and cannot fix that from the inside."""

    def test_login_exits_zero_but_sts_still_fails_refuses(self):
        result, _ = self._gate(sts_rc=1, login_rc=0, post_login_sts_rc=1)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)

    def test_the_refusal_says_why_and_names_the_config_knob(self):
        # A gate that stops a launch has to be self-explanatory, or the operator's
        # next move is to re-run the verb and get the same silence.
        result, _ = self._gate(sts_rc=1, login_rc=0, post_login_sts_rc=1)
        self.assertIn("NOT launching", result.stderr)
        self.assertIn("DS_AWS_PROFILE", result.stderr)


class FailedLoginRefuses(GateHarness):
    def test_a_dismissed_or_failed_login_refuses_the_launch(self):
        result, _ = self._gate(sts_rc=1, login_rc=1, post_login_sts_rc=1)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)


class AbsentCliWarnsRatherThanBlocking(GateHarness):
    """The gate catches an EXPIRED token, which is the measured failure. A machine
    with no `aws` at all has nothing to log into, and refusing to start a
    planning session there would make the committed verb unusable on any checkout
    without AWS."""

    def _no_aws(self):
        # An EMPTY existing directory rather than a bogus path: `command -v aws`
        # has to fail, and everything the helper uses on that branch (`print`,
        # `[[`) is a zsh builtin, so nothing else needs to be on PATH.
        empty = Path(self._tmp.name) / "empty-bin"
        empty.mkdir(exist_ok=True)
        return self._gate(stub=False, env={"PATH": str(empty)})

    def test_no_aws_on_path_still_allows_the_launch(self):
        result, _ = self._no_aws()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_no_aws_on_path_says_so(self):
        result, _ = self._no_aws()
        self.assertIn("WITHOUT cost-read", result.stderr)


class AnOldCliWarnsRatherThanBlocking(GateHarness):
    """An `aws` too old for a top-level `login` is the same "AWS is not set up
    here" shape as no `aws` at all, so it must take the same warn-and-launch
    branch. Before this, it was strictly WORSE than having no CLI: the login
    failed with `Invalid choice: 'login'`, the re-probe failed, and the verb
    refused to start — while a machine with no `aws` launched fine."""

    def test_an_old_cli_still_allows_the_launch(self):
        result, _ = self._gate(sts_rc=1, login_help_rc=1)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_an_old_cli_never_attempts_the_login(self):
        # Attempting it would print `Invalid choice` to the operator's terminal
        # for no reason — the probe exists precisely to avoid that.
        _, recorded = self._gate(sts_rc=1, login_help_rc=1)
        real_logins = [
            args for _, args in recorded if args[0] == "login" and "help" not in args
        ]
        self.assertEqual(real_logins, [], recorded)

    def test_an_old_cli_says_what_is_wrong_and_names_the_older_spelling(self):
        result, _ = self._gate(sts_rc=1, login_help_rc=1)
        self.assertIn("no top-level", result.stderr)
        self.assertIn("aws sso login", result.stderr)


class ConfiguredProfileIsPassedThrough(GateHarness):
    """The profile is untracked runtime config, never a committed constant — this
    account's profile names carry the account id."""

    def test_the_configured_profile_reaches_both_calls(self):
        _, recorded = self._gate(
            sts_rc=1, login_rc=0, post_login_sts_rc=0, env={"DS_AWS_PROFILE": "ds-dev"}
        )
        self.assertTrue(recorded, "the stub recorded no calls at all")
        # The `login help` subcommand-exists probe is excluded deliberately: it
        # asks whether a subcommand exists, which no profile is relevant to.
        credentialed = [(argc, args) for argc, args in recorded if "help" not in args]
        self.assertEqual(len(credentialed), 3, recorded)
        for argc, args in credentialed:
            # Two SEPARATE adjacent words. A single `--profile ds-dev` word — what
            # `"${profile[*]}"` produces — fails here, and is what the old
            # space-joined assertion could not see.
            self.assertEqual(argc, len(args), (argc, args))
            self.assertIn("--profile", args, args)
            self.assertEqual(args[args.index("--profile") + 1], "ds-dev", args)

    def test_no_configured_profile_passes_no_profile_flag(self):
        # Unset, the CLI resolves its own default. Passing an empty `--profile`
        # would be worse than passing none: it names a profile that cannot exist.
        _, recorded = self._gate(sts_rc=0)
        self.assertTrue(recorded, "the stub recorded no calls at all")
        for argc, args in recorded:
            self.assertNotIn("--profile", args, args)
            # The stray-empty-argument case, invisible to a space-joined record:
            # an empty array expanded with `[*]` rather than `[@]` passes one
            # empty word, which the real CLI rejects.
            self.assertNotIn("", args, f"a stray empty argument was passed: {args}")
            self.assertEqual(argc, len(args), (argc, args))


@unittest.skipUnless(shutil.which("zsh"), _NEEDS_ZSH)
class TheGateActuallyGates(unittest.TestCase):
    """Behavioral, not textual.

    `GatedVerbSurface` below greps the file, so it cannot observe the gate being
    in FORCE: moving the call to the end of `plan()` — after the launch — or
    wrapping it in `if false; then … fi` leaves those assertions green while the
    gate gates nothing. These cases run the real verb with the launcher stubbed
    and assert on whether the launch happened.
    """

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.calls = Path(self._tmp.name) / "calls"
        self.calls.touch()

    def _run_verb(self, verb, *, sts_rc, login_rc=0, post_login_sts_rc=0):
        # `_ds_seat_guard` and the launcher are both stubbed: this case is about
        # the gate's effect on the launch, and nothing else.
        script = (
            'source "%s" 2>/dev/null; %s'
            "_ds_seat_guard() { : }; "
            '_ds_daily_session() { print -r -- "1|LAUNCHED" >> "$CALLS" }; '
            "%s" % (INIT, _STUB, verb)
        )
        child_env = {**os.environ}
        for name in _SUITE_OWNED_ENV:
            child_env.pop(name, None)
        child_env.update(
            {
                "CALLS": str(self.calls),
                "STS_RC": str(sts_rc),
                "LOGIN_RC": str(login_rc),
                "POST_LOGIN_STS_RC": str(post_login_sts_rc),
                # The verb-level cases never exercise the old-CLI branch, so the
                # subcommand-exists probe always succeeds here.
                "LOGIN_HELP_RC": "0",
            }
        )
        result = subprocess.run(
            [ZSH, "-c", script],
            capture_output=True,
            text=True,
            check=False,
            env=child_env,
        )
        return result, "LAUNCHED" in self.calls.read_text(encoding="utf-8")

    def test_plan_does_not_launch_when_the_gate_refuses(self):
        result, launched = self._run_verb("plan", sts_rc=1, post_login_sts_rc=1)
        self.assertFalse(launched, "plan launched despite an unusable AWS session")
        self.assertEqual(result.returncode, 1, result.stderr)

    def test_plan_launches_when_credentials_are_valid(self):
        # The other half — a gate that refused everything would pass the case
        # above while making the verb useless.
        _, launched = self._run_verb("plan", sts_rc=0)
        self.assertTrue(launched, "plan did not launch on a valid session")

    def test_housekeeping_does_not_launch_when_the_gate_refuses(self):
        result, launched = self._run_verb("housekeeping", sts_rc=1, post_login_sts_rc=1)
        self.assertFalse(launched, "housekeeping launched despite no AWS session")
        self.assertEqual(result.returncode, 1, result.stderr)

    def test_housekeeping_launches_when_credentials_are_valid(self):
        _, launched = self._run_verb("housekeeping", sts_rc=0)
        self.assertTrue(launched, "housekeeping did not launch on a valid session")


class GatedVerbSurface(unittest.TestCase):
    """Which verbs carry the gate is an operator ruling, so pin it in the text
    rather than trusting a reading of the file."""

    def setUp(self):
        self.source = INIT.read_text(encoding="utf-8")

    def test_plan_and_housekeeping_are_gated(self):
        for verb in ("plan", "housekeeping"):
            self.assertIn(
                "_ds_aws_login '%s' || return 1" % verb,
                self.source,
                "%s lost its AWS login gate" % verb,
            )

    def test_the_gate_runs_after_the_seat_guard(self):
        # Ordering matters: the seat guard clears an `AWS_REGION` inherited from a
        # previous `task` in the same tab, and the AWS CLI reads that variable, so
        # probing first would probe the Bedrock launcher's environment.
        for verb in ("plan", "housekeeping"):
            guard = self.source.index("_ds_seat_guard '%s'" % verb)
            gate = self.source.index("_ds_aws_login '%s'" % verb)
            self.assertLess(guard, gate, "%s probes before clearing the env" % verb)

    def test_architect_is_deliberately_not_gated(self):
        # It argues design rather than reading cost data, and a browser login is a
        # poor thing to stand between the operator and a design thought.
        self.assertNotIn("_ds_aws_login 'architect'", self.source)


if __name__ == "__main__":
    unittest.main()
