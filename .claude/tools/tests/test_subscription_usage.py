#!/usr/bin/env python3
"""Unit tests for ``subscription_usage.py`` (stdlib ``unittest``; no pytest).

No keychain and no external network: the credential reader takes an injected
runner, ``fetch_usage`` an injected opener, and ``run`` injected read / fetch /
clock seams. The one socket test talks to a throwaway server on loopback, to
prove the real opener refuses a redirect rather than re-sending the bearer
token. The fixture is a live response captured on 2026-10-07, trimmed of most
of its null codename keys — two are kept so unknown keys are proven harmless.
"""

from __future__ import annotations

import contextlib
import http.server
import io
import json
import subprocess
import threading
import unittest
import urllib.error
import urllib.request

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
LEGACY_ONLY = {k: v for k, v in LIVE.items() if k != "limits"}

#: A sentinel, not a credential: the assertions prove it never reaches output.
TOKEN = "placeholder-sentinel-value"
NOW = 1_800_000_000.0
FRESH = {
    "accessToken": TOKEN,
    "expiresAt": (NOW + 3600) * 1000,
    "subscriptionType": "team",
    "rateLimitTier": "default_claude_max_5x",
}


def fake_runner(returncode, stdout="", stderr="", seen=None):
    def runner(cmd, **_):
        if seen is not None:
            seen.append(cmd)
        return subprocess.CompletedProcess(cmd, returncode, stdout, stderr)

    return runner


class ReadCredential(unittest.TestCase):
    def test_a_missing_item_is_a_reading_not_an_error(self):
        self.assertIsNone(su.read_credential(fake_runner(su.SECURITY_NOT_FOUND)))

    def test_the_oauth_block_is_read_from_claude_codes_item(self):
        seen = []
        raw = json.dumps({"claudeAiOauth": FRESH})
        block = su.read_credential(fake_runner(0, raw, seen=seen))
        self.assertEqual(block["accessToken"], TOKEN)
        self.assertEqual(
            seen,
            [["security", "find-generic-password", "-s", su.KEYCHAIN_SERVICE, "-w"]],
        )

    def test_a_malformed_item_never_echoes_or_chains_its_payload(self):
        with self.assertRaises(su.UsageError) as caught:
            su.read_credential(fake_runner(0, f"not json {TOKEN}"))
        self.assertNotIn(TOKEN, str(caught.exception))
        # A chained JSONDecodeError would carry the raw item in `.doc`.
        self.assertIsNone(caught.exception.__cause__)
        self.assertTrue(caught.exception.__suppress_context__)

    def test_an_item_without_the_oauth_block_is_refused(self):
        with self.assertRaises(su.UsageError):
            su.read_credential(fake_runner(0, json.dumps({"other": {}})))

    def test_a_block_without_a_token_is_refused(self):
        raw = json.dumps({"claudeAiOauth": {"expiresAt": 1}})
        with self.assertRaises(su.UsageError):
            su.read_credential(fake_runner(0, raw))

    def test_a_token_that_would_break_a_header_is_refused_without_echo(self):
        # http.client rejects a header value with a bare newline by raising a
        # ValueError that repeats the value — so it must never get that far.
        broken = f"{TOKEN}\nx"
        raw = json.dumps({"claudeAiOauth": dict(FRESH, accessToken=broken)})
        with self.assertRaises(su.UsageError) as caught:
            su.read_credential(fake_runner(0, raw))
        self.assertNotIn(TOKEN, str(caught.exception))

    def test_a_locked_keychain_surfaces_security_stderr(self):
        with self.assertRaises(su.UsageError) as caught:
            su.read_credential(
                fake_runner(51, stderr="User interaction is not allowed.")
            )
        self.assertIn("not allowed", str(caught.exception))


class FakeResponse:
    def __init__(self, body, status=200):
        self.status = status
        self._body = body

    def read(self):
        return self._body

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False


class FakeOpener:
    def __init__(self, outcome):
        self.outcome = outcome
        self.requests = []

    def open(self, request, timeout):
        self.requests.append((request, timeout))
        if isinstance(self.outcome, BaseException):
            raise self.outcome
        return self.outcome


class FetchUsage(unittest.TestCase):
    def fetch(self, outcome):
        opener = FakeOpener(outcome)
        return su.fetch_usage(TOKEN, 7, opener=opener), opener

    def test_a_live_body_is_parsed_and_the_headers_are_sent(self):
        result, opener = self.fetch(FakeResponse(json.dumps(LIVE).encode()))
        self.assertEqual(result, (200, LIVE, ""))
        request, timeout = opener.requests[0]
        self.assertEqual(request.full_url, su.USAGE_URL)
        self.assertEqual(request.get_header("Authorization"), f"Bearer {TOKEN}")
        self.assertEqual(request.get_header("Anthropic-beta"), su.OAUTH_BETA)
        self.assertEqual(timeout, 7)

    def test_an_http_error_is_a_result_not_a_raise(self):
        error = urllib.error.HTTPError(su.USAGE_URL, 401, "nope", {}, None)
        result, _ = self.fetch(error)
        self.assertEqual(result, (401, None, "HTTP 401"))

    def test_a_transport_error_is_unavailable(self):
        result, _ = self.fetch(urllib.error.URLError("no route"))
        self.assertEqual(result[:2], (None, None))
        self.assertIn("no route", result[2])

    def test_a_non_json_body_is_reported(self):
        result, _ = self.fetch(FakeResponse(b"<html>"))
        self.assertEqual(result, (200, None, "response body is not JSON"))

    def test_json_that_is_not_an_object_is_reported(self):
        for body in (b"[]", b"null", b'"x"'):
            with self.subTest(body=body):
                result, _ = self.fetch(FakeResponse(body))
                self.assertEqual(
                    result, (200, None, "response body is not a JSON object")
                )


class RefusesRedirects(unittest.TestCase):
    """The real opener, against a loopback server answering 302."""

    def test_a_redirect_is_surfaced_and_never_followed(self):
        landed = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path == "/landed":
                    landed.append(self.headers.get("Authorization"))
                    self.send_response(200)
                    self.end_headers()
                    return
                self.send_response(302)
                self.send_header("Location", f"http://127.0.0.1:{port}/landed")
                self.end_headers()

            def log_message(self, *_):
                pass

        server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        port = server.server_address[1]
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        request = urllib.request.Request(
            f"http://127.0.0.1:{port}/", headers={"Authorization": f"Bearer {TOKEN}"}
        )
        with self.assertRaises(urllib.error.HTTPError) as caught:
            su._OPENER.open(request, timeout=5)
        self.assertEqual(caught.exception.code, 302)
        self.assertEqual(landed, [])


class LimitRows(unittest.TestCase):
    def test_limits_array_is_parsed_with_the_scoped_model(self):
        rows = su.limit_rows(LIVE)
        self.assertEqual(
            [(r["label"], r["percent"], r["binding"], r["source"]) for r in rows],
            [
                ("session", 12, False, "limits"),
                ("weekly", 20, False, "limits"),
                ("weekly:Fable", 36, True, "limits"),
            ],
        )
        self.assertEqual(rows[0]["resets"], "2026-10-08 02:29Z")

    def test_legacy_windows_are_the_fallback_without_limits(self):
        rows = su.limit_rows(LEGACY_ONLY)
        self.assertEqual(
            [(r["label"], r["percent"], r["source"]) for r in rows],
            [("session", 12.0, "legacy"), ("weekly", 20.0, "legacy")],
        )

    def test_limits_that_yield_nothing_also_fall_back(self):
        rows = su.limit_rows(dict(LEGACY_ONLY, limits=[None, "x", 3]))
        self.assertEqual([r["source"] for r in rows], ["legacy", "legacy"])

    def test_a_surface_scoped_limit_is_labelled_by_surface(self):
        item = {"kind": "weekly_scoped", "percent": 5, "scope": {"surface": "desktop"}}
        self.assertEqual(
            su.limit_rows({"limits": [item]})[0]["label"], "weekly:desktop"
        )

    def test_reshaped_fields_degrade_instead_of_crashing(self):
        items = [
            {"kind": "weekly_scoped", "scope": "global", "percent": 1},
            {"kind": "weekly_scoped", "scope": {"model": "fable"}, "percent": 2},
            {"kind": ["session"], "percent": 3, "resets_at": 17},
        ]
        rows = su.limit_rows({"limits": items})
        self.assertEqual(
            [r["label"] for r in rows], ["weekly:scoped", "weekly:scoped", "?"]
        )
        self.assertEqual(rows[2]["resets"], "?")

    def test_an_empty_response_yields_no_rows(self):
        self.assertEqual(su.limit_rows({"limits": [], "iguana_necktie": None}), [])

    def test_reset_stamps_are_converted_to_utc(self):
        self.assertEqual(su._reset("2026-10-08T04:29:59+02:00"), "2026-10-08 02:29Z")
        self.assertEqual(su._reset("2026-10-08T02:29:59Z"), "2026-10-08 02:29Z")
        self.assertEqual(su._reset("2026-10-08T02:29:59"), "?")
        self.assertEqual(su._reset("not a time at all"), "?")

    def test_format_flags_the_binding_limit(self):
        lines = su.format_rows(su.limit_rows(LIVE))
        self.assertTrue(lines[2].startswith("weekly:Fable"))
        self.assertTrue(lines[2].endswith("BINDING"))
        self.assertIn(" 12%", lines[0])

    def test_a_non_numeric_percent_renders_as_unknown(self):
        rows = [
            {"label": "x", "percent": value, "resets": "?", "binding": False}
            for value in (True, None, "12")
        ]
        self.assertTrue(all(" ?  resets" in line for line in su.format_rows(rows)))


class Run(unittest.TestCase):
    def invoke(self, credential, fetch_result=(200, LIVE, ""), argv=("x",)):
        calls = []

        def fetch(token, timeout):
            calls.append((token, timeout))
            return fetch_result

        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = su.run(
                list(argv), read=lambda: credential, fetch=fetch, now=lambda: NOW
            )
        return code, out.getvalue(), err.getvalue(), calls

    def test_a_live_reading_reports_and_never_prints_the_token(self):
        code, out, err, calls = self.invoke(FRESH)
        self.assertEqual(code, 0)
        self.assertIn("weekly:Fable", out)
        self.assertIn("plan=team", err)
        self.assertNotIn("legacy", err)
        self.assertNotIn(TOKEN, out + err)
        self.assertEqual(calls, [(TOKEN, su.DEFAULT_TIMEOUT)])

    def test_a_legacy_only_reading_says_scoped_caps_are_hidden(self):
        code, _, err, _ = self.invoke(FRESH, fetch_result=(200, LEGACY_ONLY, ""))
        self.assertEqual(code, 0)
        self.assertIn("scoped caps not visible", err)

    def test_a_stale_token_is_reported_and_never_sent(self):
        stale = dict(FRESH, expiresAt=(NOW - 1) * 1000)
        code, out, _, calls = self.invoke(stale)
        self.assertEqual(code, 0)
        self.assertIn("stale credential", out)
        self.assertEqual(calls, [])

    def test_no_credential_is_a_clean_reading(self):
        code, out, _, calls = self.invoke(None)
        self.assertEqual(code, 0)
        self.assertIn("no subscription credential", out)
        self.assertEqual(calls, [])

    def test_a_non_200_is_unavailable_not_a_fallback(self):
        code, out, err, _ = self.invoke(FRESH, fetch_result=(404, None, "HTTP 404"))
        self.assertEqual(code, 1)
        self.assertIn("usage endpoint unavailable: HTTP 404", out)
        self.assertNotIn(TOKEN, out + err)

    def test_a_200_without_an_object_body_is_unavailable(self):
        result = (200, None, "response body is not a JSON object")
        code, out, _, _ = self.invoke(FRESH, fetch_result=result)
        self.assertEqual(code, 1)
        self.assertIn("not a JSON object", out)

    def test_no_recognizable_limits_is_a_failure_with_the_plan_line(self):
        code, out, err, _ = self.invoke(FRESH, fetch_result=(200, {"limits": []}, ""))
        self.assertEqual(code, 1)
        self.assertIn("no recognizable limits", out)
        self.assertIn("plan=team", err)

    def test_timeout_is_passed_through_and_must_be_positive(self):
        _, _, _, calls = self.invoke(FRESH, argv=("x", "--timeout", "5"))
        self.assertEqual(calls, [(TOKEN, 5)])
        for bad in ("0", "-1", "soon"):
            with self.subTest(bad=bad), self.assertRaises(SystemExit) as caught:
                self.invoke(FRESH, argv=("x", "--timeout", bad))
            self.assertEqual(caught.exception.code, 2)


if __name__ == "__main__":
    unittest.main()
