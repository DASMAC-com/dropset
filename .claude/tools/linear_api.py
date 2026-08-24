#!/usr/bin/env python3
# cspell:word newurl
"""Shared Linear GraphQL transport for the committed skill tools.

Four tools — ``trim_levers.py``, ``board_batch.py``, ``merge_tasks.py`` and
``fleet_resume.py`` — each hand-rolled the same ``_post`` helper against
``urllib``. This module is the one implementation they now share, and it exists
for two reasons beyond de-duplication.

**It refuses redirects.** ``urllib`` follows a 3xx by default and **re-sends
every header** on the follow-up request, ``Authorization`` included — so a
redirect from the endpoint would hand the Linear API key to whatever host the
``Location`` names. The exposure was never live (a hardcoded first-party HTTPS
endpoint that does not redirect, and forging the response needs TLS
interception already), but the repo had already decided this question the other
way on the Rust side: the feeds HTTP client refuses redirects as a landed
preventive measure. Two clients holding the same credential class with opposite
policies, only one of them written down, is divergence from a repo standard
rather than a considered default — so the Python side adopts the standard here,
once, for every call site.

**It is zero-echo by construction.** Nothing in this module prints a stored
body. The MCP write path echoes the whole issue description on every call, which
is a fixed per-call cost that ``patch`` does not shrink; on an accumulator issue
that compounds (44 saves for ~153k in one measured planning session, the eight
largest results all echoes of the same growing batch). Tools that write through
here read and modify inside their own process and print a one-line confirmation.

Stdlib only, and deliberately **not** a Cargo workspace member — see
``docs/conventions/skill-tooling.md``. Tests live in
``tests/test_linear_api.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request

ENDPOINT = "https://api.linear.app/graphql"

# Overall per-request timeout, so a hung endpoint can't wedge a run.
REQUEST_TIMEOUT = 30


class LinearApiError(Exception):
    """A user-facing transport or GraphQL failure.

    Callers that already have their own error class pass it as ``error=`` so a
    tool's CLI keeps surfacing one exception type. Every failure path raises
    rather than returning a sentinel, and none of them quote the credential.
    """


class _NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    """A redirect handler that declines to build the follow-up request.

    Returning ``None`` from ``redirect_request`` makes ``urllib`` fall through
    to ``HTTPDefaultErrorHandler``, which raises the 3xx as an ``HTTPError`` —
    so a redirect surfaces as a hard failure instead of silently re-sending the
    ``Authorization`` header to a new host.
    """

    def redirect_request(self, req, fp, code, msg, headers, newurl):  # noqa: D102
        return None


def build_opener() -> urllib.request.OpenerDirector:
    """An opener whose redirect handling is the refusing one above.

    ``build_opener`` drops its default ``HTTPRedirectHandler`` when passed an
    instance of a subclass, so this genuinely replaces the following behavior
    rather than stacking another handler in front of it.
    """
    return urllib.request.build_opener(_NoRedirectHandler())


# One opener for the process. Handlers here hold no per-request state, and
# building it per call would only repeat the same wiring.
_OPENER = build_opener()


def env_var(name: str, *, error: type[Exception] = LinearApiError) -> str:
    """A required environment variable, validated as printable ASCII.

    The validation is a credential-leak guard, not tidiness: a pasted key with
    an embedded newline otherwise reaches ``http.client``'s header check, which
    raises a ``ValueError`` **quoting the offending value**. ``isascii`` is
    checked alongside ``isprintable`` because a non-Latin-1 printable — a smart
    quote from a paste — passes ``isprintable`` and then fails inside the header
    encode as an uncaught ``UnicodeEncodeError`` naming the character.
    """
    value = os.environ.get(name, "").strip()
    if not value:
        raise error(f"{name} is unset — export it before running")
    if not (value.isascii() and value.isprintable()):
        raise error(f"{name} is not printable ASCII")
    return value


def post(
    api_key: str,
    query: str,
    variables: dict,
    *,
    endpoint: str = ENDPOINT,
    timeout: int = REQUEST_TIMEOUT,
    error: type[Exception] = LinearApiError,
) -> dict:
    """POST a GraphQL operation and return its ``data``.

    Transport, GraphQL and shape errors all surface as ``error`` so a CLI never
    emits a traceback — a traceback could quote the credential.
    """
    payload = json.dumps({"query": query, "variables": variables}).encode("utf-8")
    request = urllib.request.Request(
        endpoint,
        data=payload,
        headers={"Content-Type": "application/json", "Authorization": api_key},
        method="POST",
    )
    try:
        with _OPENER.open(request, timeout=timeout) as response:
            raw = response.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        if 300 <= exc.code < 400:
            # The refusal path. Named explicitly because the generic HTTP
            # message ("returned HTTP 302") reads like a server fault, and the
            # next person to hit it should not have to rediscover that the
            # refusal is deliberate.
            location = (
                exc.headers.get("Location", "<none>") if exc.headers else "<none>"
            )
            raise error(
                f"Linear API returned a {exc.code} redirect to {location!r} and this "
                "client refuses to follow it — following would re-send the "
                "Authorization header to that host. Verify the endpoint rather than "
                "enabling redirects."
            ) from exc
        detail = exc.read().decode("utf-8", errors="replace")
        raise error(f"Linear API returned HTTP {exc.code}: {detail}") from exc
    except urllib.error.URLError as exc:
        raise error(f"Linear API request failed: {exc.reason}") from exc

    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise error(f"decoding Linear GraphQL response: {exc}") from exc

    errors = parsed.get("errors")
    if errors:
        joined = "; ".join(e.get("message", "") for e in errors)
        raise error(f"Linear GraphQL error: {joined}")
    data = parsed.get("data")
    if data is None:
        raise error("Linear GraphQL response carried no data")
    return data
