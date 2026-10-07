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
carries it.

**It never refreshes the token.** A refresh rotates the refresh credential, and
Claude Code keeps its own copy in the same keychain item; refreshing here would
leave Claude Code holding a dead one. An expired token is reported as stale and
left for Claude Code to refresh on its next run.

**The windows are a rolling 5 hours and 7 days, not calendar days.** There is
no "percent of today" on this surface; a per-day figure has to be derived by
sampling, which is the bootstrap step's job, not this tool's.

**The endpoint is undocumented and has been disabled once before** (a
third-party tracker fell back to Messages-API rate-limit headers for a while),
so a non-200 or an unparseable body is reported plainly rather than papered
over with a fallback that would spend allowance to measure it.

Stdout, one line per limit, the binding one flagged::

    session        12%  resets 2026-10-08 02:29Z
    weekly         20%  resets 2026-10-09 14:59Z
    weekly:Fable   36%  resets 2026-10-09 14:59Z  BINDING

Exit codes: 0 reported (including the two expected non-readings, no credential
and a stale one, which print why); 1 endpoint unavailable or unparseable; 2
a credential that exists but cannot be read (a locked keychain, a malformed
item) or a usage error.

Usage::

    python3 .claude/tools/subscription_usage.py

Stdlib only; a Python skill-tool under ``.claude/tools/`` — deliberately not a
Cargo workspace member. Tests live in ``tests/test_subscription_usage.py``.
"""

from __future__ import annotations

import argparse
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

#: `security` exits 44 when no item matches: a fact, not a failure.
SECURITY_NOT_FOUND = 44

DEFAULT_TIMEOUT = 20


class UsageError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


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
    except (ValueError, AttributeError) as exc:
        # Deliberately not echoing the payload: it is the credential.
        raise UsageError("keychain item is not the expected JSON shape") from exc
    if not isinstance(block, dict) or not block.get("accessToken"):
        raise UsageError("keychain item carries no OAuth access token")
    return block


def is_stale(credential: dict, now: float) -> bool:
    """Whether the access token has expired (``expiresAt`` is epoch ms)."""
    expires_at = credential.get("expiresAt")
    return isinstance(expires_at, (int, float)) and expires_at / 1000 <= now


def fetch_usage(
    token: str, timeout: int = DEFAULT_TIMEOUT
) -> tuple[int | None, dict | None, str]:
    """``(status, parsed body, error)`` for one call to the usage endpoint.

    An HTTP error is a result, not a raise: a 401 or 404 is exactly what the
    caller needs to see. The error string comes from urllib, which never
    includes request headers, so the token cannot ride along in it.
    """
    request = urllib.request.Request(
        USAGE_URL,
        headers={
            "Authorization": f"Bearer {token}",
            "anthropic-beta": OAUTH_BETA,
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            status, raw = response.status, response.read()
    except urllib.error.HTTPError as exc:
        return exc.code, None, f"HTTP {exc.code}"
    except (urllib.error.URLError, http.client.HTTPException, OSError) as exc:
        return None, None, str(exc)
    try:
        return status, json.loads(raw), ""
    except ValueError:
        return status, None, "response body is not JSON"


def _reset(stamp: object) -> str:
    """``2026-10-08T02:29:59.88+00:00`` → ``2026-10-08 02:29Z``."""
    if not isinstance(stamp, str) or len(stamp) < 16:
        return "?"
    return f"{stamp[:10]} {stamp[11:16]}Z"


def limit_rows(payload: dict) -> list[dict]:
    """Normalize the response into ``{label, percent, resets, binding}`` rows.

    ``limits[]`` is the structured form and the one parsed: it is the only
    place a model-scoped cap (a per-model weekly limit) appears, and it flags
    the binding one with ``is_active``. The legacy ``five_hour`` /
    ``seven_day`` objects are the fallback for a response without it. The
    response also carries a couple of dozen null codename keys; nothing here
    reads them, so a new one cannot break the parse.
    """
    rows = []
    for item in payload.get("limits") or []:
        if not isinstance(item, dict):
            continue
        kind = item.get("kind", "")
        label = {"session": "session", "weekly_all": "weekly"}.get(kind, kind)
        scope = item.get("scope") or {}
        model = (scope.get("model") or {}).get("display_name")
        surface = scope.get("surface")
        if kind == "weekly_scoped":
            label = f"weekly:{model or surface or 'scoped'}"
        rows.append(
            {
                "label": label or "?",
                "percent": item.get("percent"),
                "resets": _reset(item.get("resets_at")),
                "binding": bool(item.get("is_active")),
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
                }
            )
    return rows


def format_rows(rows: list[dict]) -> list[str]:
    lines = []
    for row in rows:
        percent = row["percent"]
        shown = f"{percent:g}%" if isinstance(percent, (int, float)) else "?"
        flag = "  BINDING" if row["binding"] else ""
        lines.append(f"{row['label']:<14} {shown:>4}  resets {row['resets']}{flag}")
    return lines


def run(argv: list[str], read=read_credential, fetch=fetch_usage, now=time.time) -> int:
    parser = argparse.ArgumentParser(prog="subscription_usage.py")
    parser.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT)
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
        return 1
    for line in format_rows(rows):
        print(line)
    print(f"subscription-usage | {plan}", file=sys.stderr)
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except UsageError as exc:
        print(f"subscription-usage: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
