#!/usr/bin/env python3
# cspell:word testverb
"""`_ds_aws_login` — the AWS login gate the seat verbs launch behind.

Every case stubs `aws` as a shell function, so no case can trigger a real
interactive browser login. That is the whole reason this suite exists in this
shape: the behavior under test is *when* the helper decides to log in and *when
it refuses to launch*, and both are unobservable if the real CLI is in the loop.

The load-bearing assertion is `PostLoginProbeDecides` — a login can exit 0
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

from zsh_test_env import hermetic_env

INIT = Path(__file__).resolve().parents[1] / "shell" / "init.zsh"

# No case may reach a real iTerm tab, whatever it forgets to stub: inherited by
# every zsh this spawns (`iterm_api.DISABLED_ENV`).
os.environ["DS_ITERM_DISABLED"] = "1"

# Resolved once, ABSOLUTELY, because the absent-CLI cases empty `PATH` to hide
# `aws` — and a bare "zsh" in the argv would then not be found either, failing
# those cases for a reason that has nothing to do with the helper.
ZSH = shutil.which("zsh")

# Same guard and reason as the sibling shell suites: the helpers are zsh and the
# Linux CI runner ships none, so without this every case fails with
# FileNotFoundError, which says nothing about the code under test.
_NEEDS_ZSH = "the session helpers are zsh; no zsh on this machine"


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
# `[*]` substituted, every case in the suite as it then stood stayed green. The
# arity plus a delimiter that cannot occur in an AWS argument is what makes the
# boundary observable. (No case count here on purpose — the evidence does not
# depend on one, and a hard-coded number goes stale the next time a case is
# added, which it already did once.)
#
# A `default)` branch returns 42 so an unexpected subcommand fails loudly rather
# than reading as a success.
# `sso login help` is the flag-EXISTS probe and must be distinguished from a
# real `sso login`, or the probe itself would trip the repair side-effect below
# and every expired case would silently self-heal before logging in. It prints
# `HELP_TEXT` (the bare flag by default) only when `LOGIN_HELP_RC` is 0, since
# the gate reads the TEXT.
# `configure get` models the SSO-shape check the way the real CLI answers it:
# the value and exit 0 when the key is set, nothing and exit 1 when it is not.
# `SSO_SESSION` / `SSO_START_URL` hold the profile's values; empty means unset.
_STUB = (
    'aws() { local v; print -r -- "$#|${(j:|:)@}" >> "$CALLS"; '
    'if [[ "$1" == configure ]]; then '
    '[[ "$3" == sso_session ]] && v="$SSO_SESSION"; '
    '[[ "$3" == sso_start_url ]] && v="$SSO_START_URL"; '
    '[[ -n "$v" ]] && { print -r -- "$v"; return 0 }; return 1; fi; '
    'if [[ "$1" == sso && "$2" == login && "$3" == help ]]; then '
    '(( LOGIN_HELP_RC == 0 )) && print -r -- "$HELP_TEXT"; '
    "return $LOGIN_HELP_RC; fi; "
    'case "$1" in '
    "sts) return $STS_RC ;; "
    "sso) STS_RC=$POST_LOGIN_STS_RC; return $LOGIN_RC ;; "
    "*) return 42 ;; "
    "esac }; "
)


def _logins(recorded):
    """The real `sso login` calls, the flag-exists `help` probe excluded."""
    return [args for _, args in recorded if args[0] == "sso" and "help" not in args]


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
        help_text="--use-device-code",
        sso_session="dasmac",
        sso_start_url="",
        env=None,
        stub=True,
    ):
        """Run `_ds_aws_login` against the stub and return (rc, recorded calls)."""
        body = _STUB if stub else ""
        script = 'source "%s" 2>/dev/null; %s_ds_aws_login testverb' % (INIT, body)
        child_env = hermetic_env(
            f"{self.calls}.claude-home",
            env={
                "CALLS": str(self.calls),
                "STS_RC": str(sts_rc),
                "LOGIN_RC": str(login_rc),
                "POST_LOGIN_STS_RC": str(post_login_sts_rc),
                "LOGIN_HELP_RC": str(login_help_rc),
                "HELP_TEXT": help_text,
                "SSO_SESSION": sso_session,
                "SSO_START_URL": sso_start_url,
                **(env or {}),
            },
        )
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
        # The probe-before-login shape is the point: the login stops for the
        # operator, so a still-valid session that re-logs in has made the gate a tax on
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
        # `_logins` excludes the flag-exists probe, which is also spelled
        # `sso login`. Counting it would read as two logins.
        self.assertEqual(len(_logins(recorded)), 1, recorded)

    def test_the_login_is_device_code_so_the_operator_picks_the_identity(self):
        # The plain flow reuses whichever identity the default browser holds;
        # the device code is typed in a browser the operator chooses, and
        # `--no-browser` keeps the CLI from opening the default one itself.
        _, recorded = self._gate(sts_rc=1, login_rc=0, post_login_sts_rc=0)
        logins = _logins(recorded)
        self.assertEqual(len(logins), 1, recorded)
        self.assertEqual(logins[0][:2], ["sso", "login"], recorded)
        self.assertIn("--use-device-code", logins[0], recorded)
        self.assertIn("--no-browser", logins[0], recorded)

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


class NonSsoProfileRefuses(GateHarness):
    """`aws sso login` refreshes only an SSO-shaped profile; a `login_session`
    profile written by `aws login` fails it outright. The gate refuses with the
    fix rather than falling back to a second login path."""

    def _non_sso(self, **kw):
        return self._gate(sts_rc=1, sso_session="", sso_start_url="", **kw)

    def test_a_non_sso_profile_refuses_the_launch(self):
        result, _ = self._non_sso()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)

    def test_a_non_sso_profile_never_attempts_a_login(self):
        _, recorded = self._non_sso()
        self.assertEqual(_logins(recorded), [], recorded)
        self.assertNotIn("login", [args[0] for _, args in recorded], recorded)

    def test_the_refusal_names_the_fix(self):
        result, _ = self._non_sso()
        self.assertIn("NOT launching", result.stderr)
        self.assertIn("aws configure sso", result.stderr)
        self.assertIn("DS_AWS_PROFILE", result.stderr)

    def test_a_legacy_inline_sso_profile_still_logs_in(self):
        # `sso_start_url` directly in the profile stanza predates `sso_session`
        # and is still SSO-shaped.
        result, recorded = self._gate(
            sts_rc=1,
            post_login_sts_rc=0,
            sso_session="",
            sso_start_url="https://example.awsapps.com/start",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(_logins(recorded)), 1, recorded)

    def test_a_valid_non_sso_session_still_launches(self):
        # The shape check sits after the probe: a live session of any shape
        # costs nothing, so a `login_session` profile keeps working until it
        # expires.
        result, recorded = self._gate(sts_rc=0, sso_session="")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([args[0] for _, args in recorded], ["sts"], recorded)


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
    """An `aws` too old for `sso login --use-device-code` is the same "AWS is
    not set up here" shape as no `aws` at all, so it must take the same
    warn-and-launch branch. Without it, it would be strictly WORSE than having
    no CLI: the login fails on the unknown option, the re-probe fails, and the
    verb refuses to start — while a machine with no `aws` launches fine.

    The flag-exists probe deliberately runs AFTER the failed re-probe, so an
    old CLI is modelled as: probe fails, login is attempted and achieves
    nothing, re-probe fails, and only then does the gate ask whether the flag
    exists at all. The escape is reachable only for an SSO-shaped profile
    (the harness default); a non-SSO one is refused by the shape check first.
    """

    def _old_cli(self):
        # `post_login_sts_rc=1` because on a real old CLI the attempted login
        # cannot repair anything — it fails on the unknown option.
        return self._gate(sts_rc=1, post_login_sts_rc=1, login_help_rc=1)

    def test_an_old_cli_still_allows_the_launch(self):
        result, _ = self._old_cli()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_an_old_cli_asks_about_the_flag_only_after_the_reprobe(self):
        # The ordering IS the safety property: `help` renders through groff and a
        # pager and so can fail for reasons unrelated to the flag existing.
        # On the hot path that would warn-and-launch every expired session on a
        # CURRENT CLI; here it can only widen an already-failing outcome.
        _, recorded = self._old_cli()
        kinds = ["help" if "help" in args else args[0] for _, args in recorded]
        self.assertEqual(kinds, ["sts", "configure", "sso", "sts", "help"], recorded)

    def test_an_old_cli_says_what_is_wrong(self):
        result, _ = self._old_cli()
        self.assertIn("--use-device-code", result.stderr)
        self.assertIn("WITHOUT cost-read", result.stderr)

    def test_an_old_cli_tells_the_operator_to_ignore_the_stray_error(self):
        # The price of the later placement is one unknown-option error reaching
        # the terminal, so the warning has to account for it or it reads as a
        # defect.
        result, _ = self._old_cli()
        self.assertIn("unknown-option", result.stderr)

    def test_an_old_cli_on_a_non_sso_profile_is_refused_not_escaped(self):
        # The shape check runs first, so no old-CLI escape is reached for a
        # profile no login could refresh anyway.
        result, recorded = self._gate(
            sts_rc=1, post_login_sts_rc=1, login_help_rc=1, sso_session=""
        )
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("aws configure sso", result.stderr)
        self.assertEqual(_logins(recorded), [], recorded)

    def test_bolded_help_on_a_current_cli_still_refuses(self):
        # A renderer that bolds by backspace overstrike splits the flag into
        # `-\b--\b-u\bu…`. Unstripped, a current CLI whose login failed would
        # read as an old one and warn-and-launch instead of refusing.
        bolded = "".join(f"{c}\b{c}" for c in "--use-device-code")
        result, _ = self._gate(sts_rc=1, post_login_sts_rc=1, help_text=bolded)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("NOT launching", result.stderr)

    def test_a_current_cli_never_reaches_the_flag_probe(self):
        # The complement, and the one that pins the hot path: a session that
        # repairs normally must not pay for `help` at all.
        _, recorded = self._gate(sts_rc=1, post_login_sts_rc=0)
        self.assertNotIn("help", [a for _, args in recorded for a in args], recorded)


class ConfiguredProfileIsPassedThrough(GateHarness):
    """The profile is untracked runtime config, never a committed constant — this
    account's profile names carry the account id."""

    def test_the_configured_profile_reaches_both_calls(self):
        _, recorded = self._gate(
            sts_rc=1, login_rc=0, post_login_sts_rc=0, env={"DS_AWS_PROFILE": "ds-dev"}
        )
        self.assertTrue(recorded, "the stub recorded no calls at all")
        # The `sso login help` flag-exists probe is excluded deliberately: it
        # asks whether a flag exists, which no profile is relevant to. The
        # `configure get` shape check is included — it must read the SAME
        # profile the probe and the login use.
        credentialed = [(argc, args) for argc, args in recorded if "help" not in args]
        self.assertEqual(
            [args[0] for _, args in credentialed],
            ["sts", "configure", "sso", "sts"],
            recorded,
        )
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
        # the gate's effect on the launch, and nothing else. `plan` reaches
        # `_ds_session` directly (its cycle loop computes the id). The substrate dir is pointed at
        # this case's temp dir, since the cycle loop clears its marker there
        # and must never touch the base checkout's live one.
        script = (
            'source "%s" 2>/dev/null; %s'
            '_DS_SUBSTRATE_DIR="$CALLS.state"; '
            "_ds_seat_guard() { : }; "
            # The scheduler's own gate, on a warm day; the pre-launch's gate is
            # `test_plan_cycle_loop.PreLaunch`'s.
            "_ds_plan_fresh_day() { return 1 }; "
            '_ds_session() { print -r -- "1|LAUNCHED" >> "$CALLS" }; '
            "%s" % (INIT, _STUB, verb)
        )
        child_env = hermetic_env(
            f"{self.calls}.claude-home",
            env={
                "CALLS": str(self.calls),
                "STS_RC": str(sts_rc),
                "LOGIN_RC": str(login_rc),
                "POST_LOGIN_STS_RC": str(post_login_sts_rc),
                # The verbs refuse an unset model tier before this gate is
                # reached, so a placeholder tier keeps the case on the gate.
                "DS_MODEL_ADVISOR": "judge-model[1m]",
                "DS_MODEL_EXECUTOR": "work-model[1m]",
                # The verb-level cases never exercise the old-CLI or non-SSO
                # branches, so the flag-exists probe always succeeds and the
                # profile is SSO-shaped here.
                "LOGIN_HELP_RC": "0",
                "HELP_TEXT": "--use-device-code",
                "SSO_SESSION": "dasmac",
                "SSO_START_URL": "",
            },
        )
        result = subprocess.run(
            [ZSH, "-c", script],
            capture_output=True,
            text=True,
            check=False,
            env=child_env,
        )
        return result, "LAUNCHED" in self.calls.read_text(encoding="utf-8")

    # `plan anthropic`, not the Bedrock default: the Bedrock gate refuses with no
    # bearer token (CI has none), which would make the refusal case pass for
    # the wrong reason and the launch case fail. The AWS gate is the same on
    # both substrates, and the seat guard is stubbed above.
    def test_plan_does_not_launch_when_the_gate_refuses(self):
        result, launched = self._run_verb(
            "plan anthropic", sts_rc=1, post_login_sts_rc=1
        )
        self.assertFalse(launched, "plan launched despite an unusable AWS session")
        self.assertEqual(result.returncode, 1, result.stderr)

    def test_plan_launches_when_credentials_are_valid(self):
        # The other half — a gate that refused everything would pass the case
        # above while making the verb useless.
        _, launched = self._run_verb("plan anthropic", sts_rc=0)
        self.assertTrue(launched, "plan did not launch on a valid session")


class GatedVerbSurface(unittest.TestCase):
    """Which verbs carry the gate is an operator ruling, so pin it in the text
    rather than trusting a reading of the file."""

    def setUp(self):
        self.source = INIT.read_text(encoding="utf-8")

    def test_plan_is_gated(self):
        # `plan` spells the refusal as an `if !` so it can undo a Bedrock
        # launch's exports first.
        self.assertIn("if ! _ds_aws_login 'plan'; then", self.source)

    def test_the_gate_runs_after_the_seat_guard(self):
        # Ordering matters: the seat guard clears an `AWS_REGION` inherited from a
        # previous `task` in the same tab, and the AWS CLI reads that variable, so
        # probing first would probe the Bedrock launcher's environment. `plan`
        # reaches the guard through `_ds_substrate_enter` in the shared
        # `_ds_daily_seat` body, which also carries its `bedrock` override.
        guard = self.source.index('_ds_substrate_enter "$substrate" "$kind"')
        gate = self.source.index("if ! _ds_aws_login 'plan'; then")
        self.assertLess(guard, gate, "plan probes before clearing the env")
        # The pre-launch probes in the operator's own environment too: it
        # clears inherited Bedrock exports before its gate.
        body = self.source.index("_ds_plan_prelaunch() {")
        unset = self.source.index("_ds_substrate_unset", body)
        early = self.source.index("_ds_aws_login 'plan' || return 1", body)
        self.assertLess(unset, early, "the pre-launch probes before clearing")

    def test_architect_is_deliberately_not_gated(self):
        # It argues design rather than reading cost data, and a browser login is a
        # poor thing to stand between the operator and a design thought.
        self.assertNotIn("_ds_aws_login 'architect'", self.source)

    def test_curate_is_deliberately_not_gated(self):
        # The curator reads no cost data: the gate sits behind the scheduler's
        # kind in the shared body, and nowhere else.
        self.assertNotIn("_ds_aws_login 'curate'", self.source)
        # The shared body's gate call sits INSIDE the scheduler-only guard,
        # before its fi; the only other is the scheduler's own pre-launch.
        guard = self.source.index('if [[ "$kind" == plan ]]; then')
        gate = self.source.index("if ! _ds_aws_login 'plan'; then")
        close = self.source.index("\n  fi\n", guard)
        self.assertEqual(self.source.count("_ds_aws_login 'plan'"), 2)
        self.assertIn("_ds_aws_login 'plan' || return 1", self.source)
        self.assertLess(guard, gate)
        self.assertLess(gate, close)


if __name__ == "__main__":
    unittest.main()
