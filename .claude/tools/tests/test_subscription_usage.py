#!/usr/bin/env python3
"""Unit tests for ``subscription_usage.py`` (stdlib ``unittest``; no pytest).

No keychain and no network: the credential reader takes an injected runner,
and ``run`` takes injected read / fetch / clock seams. The fixture is a live
response captured on 2026-10-07, trimmed of most of its null codename keys —
one is kept so an unknown key is proven harmless.
"""

from __future__ import annotations

import contextlib
import io
import json
import subprocess
import unittest

import subscription_usage as su

#: Live `/api/oauth/usage` response, 2026-10-07 (codename keys trimmed).
LIVE = {
    "five_hour": {"utilization": 12.0, "resets_at": "2026-10-08T02:29:59.884541+00:00"},
    "seven_day": {"utilization": 20.0, "resets_at": "2026-10-09T14:59:59.884561+00:00"},
    "seven_day_opus": None,
    "iguana_necktie": None,
    "extra_usage": {"is_enabled": False, "user_disabled": True},
    "limits": [
        {
            "kind": "session",
            "group": "session",
            "percent": 12,
            "severity": "normal",
            "resets_at": "2026-10-08T02:29:59.884541+00:00",
            "scope": None,
            "is_active": False,
        },
        {
            "kind": "weekly_all",
            "group": "weekly",
            "percent": 20,
            "severity": "normal",
            "resets_at": "2026-10-09T14:59:59.884561+00:00",
            "scope": None,
            "is_active": False,
        },
        {
            "kind": "weekly_scoped",
            "group": "weekly",
            "percent": 36,
            "severity": "normal",
            "resets_at": "2026-10-09T14:59:59.884742+00:00",
            "scope": {"model": {"id": None, "display_name": "Fable"}, "surface": None},
            "is_active": True,
        },
    ],
}

#: A sentinel, not a credential: the assertions prove it never reaches output.
TOKEN = "placeholder-sentinel-value"
NOW = 1_800_000_000.0
FRESH = {
    "accessToken": TOKEN,
    "expiresAt": (NOW + 3600) * 1000,
    "subscriptionType": "team",
    "rateLimitTier": "default_claude_max_5x",
}


def fake_runner(returncode, stdout="", stderr=""):
    def runner(cmd, **_):
        return subprocess.CompletedProcess(cmd, returncode, stdout, stderr)

    return runner


class ReadCredential(unittest.TestCase):
    def test_a_missing_item_is_a_reading_not_an_error(self):
        self.assertIsNone(su.read_credential(fake_runner(su.SECURITY_NOT_FOUND)))

    def test_the_oauth_block_is_returned(self):
        raw = json.dumps({"claudeAiOauth": FRESH})
        self.assertEqual(su.read_credential(fake_runner(0, raw))["accessToken"], TOKEN)

    def test_a_malformed_item_never_echoes_its_payload(self):
        with self.assertRaises(su.UsageError) as caught:
            su.read_credential(fake_runner(0, f"not json {TOKEN}"))
        self.assertNotIn(TOKEN, str(caught.exception))

    def test_a_block_without_a_token_is_refused(self):
        raw = json.dumps({"claudeAiOauth": {"expiresAt": 1}})
        with self.assertRaises(su.UsageError):
            su.read_credential(fake_runner(0, raw))

    def test_a_locked_keychain_surfaces_security_stderr(self):
        with self.assertRaises(su.UsageError) as caught:
            su.read_credential(
                fake_runner(51, stderr="User interaction is not allowed.")
            )
        self.assertIn("not allowed", str(caught.exception))


class LimitRows(unittest.TestCase):
    def test_limits_array_is_parsed_with_the_scoped_model(self):
        rows = su.limit_rows(LIVE)
        self.assertEqual(
            [(r["label"], r["percent"], r["binding"]) for r in rows],
            [("session", 12, False), ("weekly", 20, False), ("weekly:Fable", 36, True)],
        )
        self.assertEqual(rows[0]["resets"], "2026-10-08 02:29Z")

    def test_legacy_windows_are_the_fallback_without_limits(self):
        legacy = {k: v for k, v in LIVE.items() if k != "limits"}
        rows = su.limit_rows(legacy)
        self.assertEqual(
            [(r["label"], r["percent"]) for r in rows],
            [("session", 12.0), ("weekly", 20.0)],
        )

    def test_an_empty_response_yields_no_rows(self):
        self.assertEqual(su.limit_rows({"limits": [], "iguana_necktie": None}), [])

    def test_format_flags_the_binding_limit(self):
        lines = su.format_rows(su.limit_rows(LIVE))
        self.assertTrue(lines[2].startswith("weekly:Fable"))
        self.assertTrue(lines[2].endswith("BINDING"))
        self.assertIn(" 12%", lines[0])


class Run(unittest.TestCase):
    def invoke(self, credential, fetch_result=(200, LIVE, "")):
        calls = []

        def fetch(token, timeout):
            calls.append(token)
            return fetch_result

        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = su.run(["x"], read=lambda: credential, fetch=fetch, now=lambda: NOW)
        return code, out.getvalue() + err.getvalue(), calls

    def test_a_live_reading_reports_and_never_prints_the_token(self):
        code, output, calls = self.invoke(FRESH)
        self.assertEqual(code, 0)
        self.assertIn("weekly:Fable", output)
        self.assertIn("plan=team", output)
        self.assertNotIn(TOKEN, output)
        self.assertEqual(calls, [TOKEN])

    def test_a_stale_token_is_reported_and_never_sent(self):
        stale = dict(FRESH, expiresAt=(NOW - 1) * 1000)
        code, output, calls = self.invoke(stale)
        self.assertEqual(code, 0)
        self.assertIn("stale credential", output)
        self.assertEqual(calls, [])

    def test_no_credential_is_a_clean_reading(self):
        code, output, calls = self.invoke(None)
        self.assertEqual(code, 0)
        self.assertIn("no subscription credential", output)
        self.assertEqual(calls, [])

    def test_a_non_200_is_unavailable_not_a_fallback(self):
        code, output, _ = self.invoke(FRESH, fetch_result=(404, None, "HTTP 404"))
        self.assertEqual(code, 1)
        self.assertIn("usage endpoint unavailable: HTTP 404", output)
        self.assertNotIn(TOKEN, output)


if __name__ == "__main__":
    unittest.main()
