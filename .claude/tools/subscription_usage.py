#!/usr/bin/env python3
"""Report the Claude subscription allowance: session, weekly, and scoped limits.

The seat side of the fleet cost picture. Bedrock spend is priced from
transcripts and reconciled against the Marketplace bill; the subscription has
no bill, only an allowance, and this reads it from the same undocumented
endpoint Claude Code's own ``/usage`` view and third-party menu-bar trackers
use::

    GET https://api.anthropic.com/api/oauth/usage
    Authorization: Bearer <Claude Code OAuth access token>
    anthropic-beta: oauth-2025-04-20

**The token is Claude Code's, read from the macOS login keychain, and is never
printed.** Nothing this tool writes — stdout, stderr, or an exception message —
carries it. Two guards make that hold rather than merely hold today: a token
that is not one printable word is refused before it reaches a header (http.client
echoes a rejected header value in its error), and **redirects are refused**,
because urllib re-sends every request header, ``Authorization`` included, to
whatever host a 30x names.

**It never refreshes the token.** A refresh rotates the refresh credential, and
Claude Code keeps its own copy in the same keychain item; refreshing here would
leave Claude Code holding a dead one. An expired token is reported as stale and
left for Claude Code to refresh on its next run.

**The windows are a rolling 5 hours and 7 days, not calendar days.** There is
no "percent of today" on this surface; a per-day figure has to be derived by
sampling, which is the bootstrap step's job, not this tool's.

**The endpoint is undocumented and has been disabled once before** (a
third-party tracker fell back to Messages-API rate-limit headers for a while),
so a non-200, a redirect, or a body that is not a JSON object is reported
plainly rather than papered over with a fallback that would spend allowance to
measure it.

Stdout, one line per limit, the binding one flagged::

    session        12%  resets 2026-10-08 02:29Z
    weekly         20%  resets 2026-10-09 14:59Z
    weekly:Fable   36%  resets 2026-10-09 14:59Z  BINDING

Exit codes: 0 reported (including the two expected non-readings, no credential
and a stale one, which print why); 1 endpoint unavailable or unparseable; 2
a credential that exists but cannot be read (a locked keychain, a malformed
item) or a usage error.

Usage::

    python3 .claude/tools/subscription_usage.py [--timeout SECONDS]

Stdlib only; a Python skill-tool under ``.claude/tools/`` — deliberately not a
Cargo workspace member. Tests live in ``tests/test_subscription_usage.py``.
"""

from __future__ import annotations

import argparse
import datetime
import http.client
import json
import subprocess
import sys
import time
import urllib.error
import urllib.request

USAGE_URL = "https://api.anthropic.com/api/oauth/usage"
OAUTH_BETA = "oauth-2025-04-20"

#: The keychain item Claude Code stores its subscription login under.
KEYCHAIN_SERVICE = "Claude Code-credentials"

#: `security` exits 44 when no item matches (measured 2026-10-07): a fact, not
#: a failure.
SECURITY_NOT_FOUND = 44

DEFAULT_TIMEOUT = 20


class UsageError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


class _RefuseRedirect(urllib.request.HTTPRedirectHandler):
    """Never follow a redirect: the 30x surfaces as an ``HTTPError`` instead.

    The stock handler copies every request header except the content ones onto
    the new request, with no host or scheme check, so following a redirect
    would hand the bearer token to the ``Location`` host — over plain http, if
    that is what it names. A moved endpoint is a fact to report, not to chase.
    """

    def redirect_request(self, *_args):
        return None


_OPENER = urllib.request.build_opener(_RefuseRedirect)


def _is_header_safe(token: object) -> bool:
    return (
        isinstance(token, str)
        and bool(token)
        and all(c.isprintable() and not c.isspace() for c in token)
    )


def read_credential(runner=subprocess.run) -> dict | None:
    """Claude Code's OAuth credential block, or ``None`` when there is none.

    ``None`` is the expected answer on a Bedrock-only machine, so it is a
    reading, not an error. ``runner`` is injectable so tests never touch the
    real keychain.
    """
    result = runner(
        ["security", "find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"],
        capture_output=True,
        text=True,
    )
    if result.returncode == SECURITY_NOT_FOUND:
        return None
    if result.returncode != 0:
        # stderr from `security` names the failure, never the secret.
        raise UsageError(
            f"keychain read failed (exit {result.returncode}): {result.stderr.strip()}"
        )
    try:
        block = json.loads(result.stdout).get("claudeAiOauth")
    except (ValueError, AttributeError):
        # Deliberately not echoing or chaining the payload: it is the
        # credential, refresh token included.
        raise UsageError("keychain item is not the expected JSON shape") from None
    if not isinstance(block, dict) or not block.get("accessToken"):
        raise UsageError("keychain item carries no OAuth access token")
    if not _is_header_safe(block["accessToken"]):
        raise UsageError("keychain access token is not a single printable word")
    return block


def is_stale(credential: dict, now: float) -> bool:
    """Whether the access token has expired (``expiresAt`` is epoch ms).

    An absent or non-numeric expiry reads as fresh: the token is tried, and an
    expired one comes back as a 401, which is reported plainly.
    """
    expires_at = credential.get("expiresAt")
    return isinstance(expires_at, (int, float)) and expires_at / 1000 <= now


def fetch_usage(
    token: str, timeout: int = DEFAULT_TIMEOUT, opener=None
) -> tuple[int | None, dict | None, str]:
    """``(status, parsed body, error)`` for one call to the usage endpoint.

    An HTTP error — a 30x included, since redirects are refused — is a result,
    not a raise: a 401 or 404 is exactly what the caller needs to see. The
    error string comes from urllib, which never includes request headers, so
    the token cannot ride along in it. The body is ``None`` whenever it is not
    a JSON object, with ``error`` saying why.
    """
    request = urllib.request.Request(
        USAGE_URL,
        headers={
            "Authorization": f"Bearer {token}",
            "anthropic-beta": OAUTH_BETA,
        },
    )
    try:
        with (opener or _OPENER).open(request, timeout=timeout) as response:
            status, raw = response.status, response.read()
    except urllib.error.HTTPError as exc:
        return exc.code, None, f"HTTP {exc.code}"
    except (http.client.HTTPException, OSError) as exc:
        return None, None, str(exc) or type(exc).__name__
    try:
        payload = json.loads(raw)
    except ValueError:
        return status, None, "response body is not JSON"
    if not isinstance(payload, dict):
        return status, None, "response body is not a JSON object"
    return status, payload, ""


def _reset(stamp: object) -> str:
    """``2026-10-08T02:29:59.88+00:00`` → ``2026-10-08 02:29Z``, in UTC."""
    if not isinstance(stamp, str):
        return "?"
    if stamp.endswith("Z"):
        stamp = stamp[:-1] + "+00:00"
    try:
        moment = datetime.datetime.fromisoformat(stamp)
    except ValueError:
        return "?"
    if moment.tzinfo is None:
        return "?"
    return moment.astimezone(datetime.timezone.utc).strftime("%Y-%m-%d %H:%MZ")


def _as_dict(value: object) -> dict:
    return value if isinstance(value, dict) else {}


def limit_rows(payload: dict) -> list[dict]:
    """Normalize the response into ``{label, percent, resets, binding, source}``.

    ``limits[]`` is the structured form and the one parsed: it is the only
    place a model-scoped cap (a per-model weekly limit) appears, and it flags
    the binding one with ``is_active``. The legacy ``five_hour`` /
    ``seven_day`` objects are the fallback when it yields nothing; those rows
    carry ``source: "legacy"`` so the caller can say a scoped cap may be
    hidden. The response also carries a couple of dozen null codename keys;
    nothing here reads them, and every field read is type-checked, so a new or
    reshaped key degrades to ``?`` rather than a traceback.
    """
    rows = []
    for item in payload.get("limits") or []:
        if not isinstance(item, dict):
            continue
        kind = item.get("kind")
        kind = kind if isinstance(kind, str) else ""
        label = {"session": "session", "weekly_all": "weekly"}.get(kind, kind)
        scope = _as_dict(item.get("scope"))
        model = _as_dict(scope.get("model")).get("display_name")
        surface = scope.get("surface")
        if kind == "weekly_scoped":
            names = [n for n in (model, surface) if isinstance(n, str) and n]
            label = f"weekly:{names[0] if names else 'scoped'}"
        rows.append(
            {
                "label": label or "?",
                "percent": item.get("percent"),
                "resets": _reset(item.get("resets_at")),
                "binding": item.get("is_active") is True,
                "source": "limits",
            }
        )
    if rows:
        return rows
    for key, label in (("five_hour", "session"), ("seven_day", "weekly")):
        window = payload.get(key)
        if isinstance(window, dict):
            rows.append(
                {
                    "label": label,
                    "percent": window.get("utilization"),
                    "resets": _reset(window.get("resets_at")),
                    "binding": False,
                    "source": "legacy",
                }
            )
    return rows


def format_rows(rows: list[dict]) -> list[str]:
    lines = []
    for row in rows:
        percent = row["percent"]
        numeric = isinstance(percent, (int, float)) and not isinstance(percent, bool)
        shown = f"{percent:g}%" if numeric else "?"
        flag = "  BINDING" if row["binding"] else ""
        lines.append(f"{row['label']:<14} {shown:>4}  resets {row['resets']}{flag}")
    return lines


def _positive_int(text: str) -> int:
    value = int(text)
    if value <= 0:
        raise argparse.ArgumentTypeError("must be a positive number of seconds")
    return value


def run(argv: list[str], read=read_credential, fetch=fetch_usage, now=time.time) -> int:
    parser = argparse.ArgumentParser(prog="subscription_usage.py")
    parser.add_argument("--timeout", type=_positive_int, default=DEFAULT_TIMEOUT)
    args = parser.parse_args(argv[1:])

    credential = read()
    if credential is None:
        print(
            "no subscription credential: Claude Code is not logged in on this machine"
        )
        return 0
    plan = (
        f"plan={credential.get('subscriptionType') or '?'} "
        f"tier={credential.get('rateLimitTier') or '?'}"
    )
    if is_stale(credential, now()):
        print(
            "stale credential: the access token has expired; Claude Code "
            "refreshes it on its next run (this tool never does)"
        )
        print(f"subscription-usage | {plan}", file=sys.stderr)
        return 0

    status, payload, error = fetch(credential["accessToken"], args.timeout)
    if status != 200 or payload is None:
        print(f"usage endpoint unavailable: {error or f'HTTP {status}'}")
        print(f"subscription-usage | {plan}", file=sys.stderr)
        return 1
    rows = limit_rows(payload)
    if not rows:
        print("usage endpoint answered with no recognizable limits")
        print(f"subscription-usage | {plan}", file=sys.stderr)
        return 1
    for line in format_rows(rows):
        print(line)
    summary = f"subscription-usage | {plan}"
    if rows[0]["source"] == "legacy":
        summary += " | limits[] absent: legacy windows only, scoped caps not visible"
    print(summary, file=sys.stderr)
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except UsageError as exc:
        print(f"subscription-usage: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
