#!/usr/bin/env python3
"""Unit tests for linear_api.py.

The property worth pinning hardest is the **redirect refusal**. ``urllib``
follows a 3xx by default and re-sends every header on the follow-up request,
``Authorization`` included, so a redirect would hand the Linear API key to
whatever host the ``Location`` names. That is a policy this module enforces, not
an incidental default, so it is tested at the handler level (the opener really
is wired with the refusing handler) as well as at the surface (a 3xx raises).

Nothing here touches the network.
"""

from __future__ import annotations

import io
import json
import os
import sys
import unittest
import urllib.error
import urllib.request
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import linear_api  # noqa: E402


def _response(payload: dict) -> mock.MagicMock:
    """A context-manager handle whose ``read`` yields ``payload`` as JSON."""
    handle = mock.MagicMock()
    handle.__enter__.return_value = io.BytesIO(json.dumps(payload).encode())
    return handle


class RedirectRefusalTests(unittest.TestCase):
    def test_the_opener_is_wired_with_the_refusing_handler(self):
        opener = linear_api.build_opener()
        redirect_handlers = [
            h
            for h in opener.handlers
            if isinstance(h, urllib.request.HTTPRedirectHandler)
        ]
        # Exactly one, and it must be ours — `build_opener` drops its default
        # HTTPRedirectHandler only because we pass a subclass instance. If that
        # ever regressed, the default would silently follow redirects again.
        self.assertEqual(len(redirect_handlers), 1)
        self.assertIsInstance(redirect_handlers[0], linear_api._NoRedirectHandler)

    def test_redirect_request_declines_to_build_a_follow_up(self):
        # Returning None is the whole mechanism: urllib then falls through to
        # HTTPDefaultErrorHandler, which raises the 3xx instead of following it.
        handler = linear_api._NoRedirectHandler()
        self.assertIsNone(
            handler.redirect_request(
                mock.Mock(), mock.Mock(), 302, "Found", {}, "https://evil.example/"
            )
        )

    def test_a_redirect_is_refused_with_a_message_naming_the_location(self):
        error = urllib.error.HTTPError(
            linear_api.ENDPOINT,
            302,
            "Found",
            {"Location": "https://evil.example/"},
            None,
        )
        with mock.patch.object(linear_api, "_OPENER") as opener:
            opener.open.side_effect = error
            with self.assertRaises(linear_api.LinearApiError) as caught:
                linear_api.post("key", "query", {})
        message = str(caught.exception)
        self.assertIn("evil.example", message)
        self.assertIn("refuses to follow", message)
        # The reason has to be in the message: a bare "HTTP 302" reads as a
        # server fault rather than a deliberate policy.
        self.assertIn("Authorization", message)

    def test_the_credential_is_never_quoted_in_a_redirect_refusal(self):
        error = urllib.error.HTTPError(
            linear_api.ENDPOINT,
            307,
            "Redirect",
            {"Location": "https://evil.example/"},
            None,
        )
        with mock.patch.object(linear_api, "_OPENER") as opener:
            opener.open.side_effect = error
            with self.assertRaises(linear_api.LinearApiError) as caught:
                linear_api.post("lin_api_SECRET", "query", {})
        self.assertNotIn("SECRET", str(caught.exception))


class PostTests(unittest.TestCase):
    def test_data_is_returned_on_success(self):
        with mock.patch.object(linear_api, "_OPENER") as opener:
            opener.open.return_value = _response({"data": {"issue": {"id": "x"}}})
            data = linear_api.post("key", "query", {})
        self.assertEqual(data, {"issue": {"id": "x"}})

    def test_the_authorization_header_is_sent(self):
        with mock.patch.object(linear_api, "_OPENER") as opener:
            opener.open.return_value = _response({"data": {}})
            linear_api.post("key-123", "query", {})
        request = opener.open.call_args[0][0]
        self.assertEqual(request.get_header("Authorization"), "key-123")
        self.assertEqual(request.get_method(), "POST")

    def test_a_graphql_error_is_surfaced(self):
        with mock.patch.object(linear_api, "_OPENER") as opener:
            opener.open.return_value = _response(
                {"errors": [{"message": "bad filter"}]}
            )
            with self.assertRaises(linear_api.LinearApiError) as caught:
                linear_api.post("key", "query", {})
        self.assertIn("bad filter", str(caught.exception))

    def test_a_missing_data_key_is_an_error_not_an_empty_dict(self):
        with mock.patch.object(linear_api, "_OPENER") as opener:
            opener.open.return_value = _response({})
            with self.assertRaises(linear_api.LinearApiError):
                linear_api.post("key", "query", {})

    def test_undecodable_json_is_an_error(self):
        handle = mock.MagicMock()
        handle.__enter__.return_value = io.BytesIO(b"not json")
        with mock.patch.object(linear_api, "_OPENER") as opener:
            opener.open.return_value = handle
            with self.assertRaises(linear_api.LinearApiError):
                linear_api.post("key", "query", {})

    def test_a_non_redirect_http_error_carries_the_detail(self):
        error = urllib.error.HTTPError(
            linear_api.ENDPOINT, 401, "Unauthorized", {}, io.BytesIO(b"nope")
        )
        with mock.patch.object(linear_api, "_OPENER") as opener:
            opener.open.side_effect = error
            with self.assertRaises(linear_api.LinearApiError) as caught:
                linear_api.post("key", "query", {})
        self.assertIn("401", str(caught.exception))
        self.assertIn("nope", str(caught.exception))

    def test_a_transport_failure_is_surfaced(self):
        with mock.patch.object(linear_api, "_OPENER") as opener:
            opener.open.side_effect = urllib.error.URLError("no route")
            with self.assertRaises(linear_api.LinearApiError) as caught:
                linear_api.post("key", "query", {})
        self.assertIn("no route", str(caught.exception))

    def test_a_callers_error_class_is_used_when_given(self):
        class Custom(Exception):
            pass

        with mock.patch.object(linear_api, "_OPENER") as opener:
            opener.open.side_effect = urllib.error.URLError("no route")
            with self.assertRaises(Custom):
                linear_api.post("key", "query", {}, error=Custom)


class EnvVarTests(unittest.TestCase):
    def test_a_set_variable_is_returned_stripped(self):
        with mock.patch.dict(os.environ, {"X": "  value  "}):
            self.assertEqual(linear_api.env_var("X"), "value")

    def test_an_unset_variable_raises(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(linear_api.LinearApiError):
                linear_api.env_var("X")

    def test_an_embedded_newline_is_refused_before_it_reaches_the_header(self):
        # http.client's header validation would raise a ValueError quoting the
        # offending value — i.e. leaking the credential into a traceback.
        with mock.patch.dict(os.environ, {"X": "a\nb"}):
            with self.assertRaises(linear_api.LinearApiError):
                linear_api.env_var("X")

    def test_a_non_ascii_printable_is_refused(self):
        # A smart quote from a paste passes isprintable() and then fails inside
        # the header encode as an uncaught UnicodeEncodeError naming the char.
        with mock.patch.dict(os.environ, {"X": "key’s"}):
            with self.assertRaises(linear_api.LinearApiError):
                linear_api.env_var("X")

    def test_a_callers_error_class_is_used_when_given(self):
        class Custom(Exception):
            pass

        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(Custom):
                linear_api.env_var("X", error=Custom)


if __name__ == "__main__":
    unittest.main()
