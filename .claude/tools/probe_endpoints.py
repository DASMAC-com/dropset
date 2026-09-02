#!/usr/bin/env python3
"""Probe a list of HTTP endpoints and report their shape, not their bodies.

**Be honest about what this saves: prompt-churn and wall-clock, NOT tokens.**
Writing response bodies to a file already keeps them out of context — one
session's fourteen hand-written `curl` calls totalled ~391 tokens — so selling
this as a token win would be false. What it removes is that every candidate
endpoint costs its own permission prompt and its own hand-composed format
string, and each of those is a fresh opportunity to omit the timeout or forget
the redirect and drop a 200KB body into the transcript.

**The redirect column is the point.** The feeds HTTP client refuses redirects
by standing policy, so *reachable* does not imply *reachable by our client* —
an endpoint that only resolves through a 301 is, for our purposes, down. The
session that motivated this tool hit exactly that case (a central-bank CSV
endpoint answering only through a redirect) and caught it **only** because the
hand-written format string happened to include the redirect count. A tool makes
that structural instead of lucky.

Bodies go to ``--out-dir`` and **never to stdout**, one file per endpoint named
after its label. Stdout carries one table row per endpoint:

    label  status  bytes  content-type  final-url  [REDIRECTED(n)] [TRUNCATED(…)]

Redirection and truncation are **inline flags rather than columns**, because a
column is something a reader skims past and both of these are facts the caller
has to act on. The reported URL has its query string replaced by ``?…``: this
repo's feed venues authenticate by query parameter, so the URL is the one place
a probe could put a credential in the transcript.

Usage::

    python3 .claude/tools/probe_endpoints.py --out-dir /tmp/probe \\
        --url ecb=https://example.invalid/rates.csv \\
        --url boe=https://example.invalid/boe.json

    python3 .claude/tools/probe_endpoints.py --out-dir /tmp/probe \\
        --url-file endpoints.txt

Stdlib only; a Python skill-tool under ``.claude/tools/`` — deliberately not a
Cargo workspace member. Tests live in ``tests/test_probe_endpoints.py``.
"""

# `newurl` is urllib's own parameter name in `redirect_request`, so overriding
# that method has to spell it exactly.
# cspell:word newurl
# cspell:word APFS

from __future__ import annotations

import argparse
import http.client
import os
import re
import sys
import urllib.error
import urllib.request

#: Per-request timeout. A probe is a reachability question, so a slow endpoint
#: is itself an answer — waiting longer does not make it a better one.
DEFAULT_TIMEOUT = 20

#: Bytes read from any one response. A probe needs the shape, not the payload;
#: past this the file is a data dump the caller did not ask for.
MAX_BODY_BYTES = 2_000_000

#: Label characters allowed, so a label can safely become a filename.
_SAFE_LABEL = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_.")


class ProbeError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


class _CountingRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Follows redirects but counts them, so the caller learns there were any.

    Following and *reporting* beats refusing here: refusing would answer
    "unreachable" for an endpoint that is merely relocated, which is a different
    fact and the one the caller most needs distinguished.
    """

    def __init__(self) -> None:
        self.count = 0

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        self.count += 1
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def parse_label_url(raw: str) -> tuple[str, str]:
    """Split a ``label=url`` pair, validating the label as filename-safe."""
    label, sep, url = raw.partition("=")
    # Redacted on the ARGV path too, not only in the result rows. These fire on
    # a typo — a mistyped scheme, a dropped `=` — which is exactly when a keyed
    # venue URL is still on the command line, and echoing it whole would put the
    # key in stderr.
    if not sep or not label or not url:
        raise ProbeError(f"expected label=url, got {redact_query(raw)!r}")
    bad = set(label) - _SAFE_LABEL
    if bad:
        raise ProbeError(
            f"label {label!r} has characters that cannot be a filename: "
            f"{''.join(sorted(bad))}"
        )
    if not url.startswith(("http://", "https://")):
        raise ProbeError(
            f"{label}: only http(s) URLs are probed, got {redact_query(url)!r}"
        )
    return label, url


def probe_one(label: str, url: str, out_dir: str, timeout: int) -> dict:
    """Fetch one endpoint, write its body, and return the row describing it."""
    handler = _CountingRedirectHandler()
    opener = urllib.request.build_opener(handler)
    row = {
        "label": label,
        "url": url,
        "status": None,
        "redirects": 0,
        "bytes": 0,
        "content_type": "",
        "final_url": "",
        "error": "",
        "truncated": False,
    }
    # Read ONE byte past the cap so truncation is detectable rather than
    # inferred from a suspiciously round byte count. A partial file on disk is
    # indistinguishable from a complete one, and the downstream step parses the
    # file rather than reading the row — so a silent cap is the more dangerous
    # of the two facts this tool reports. `localnet_psql.py` states the house
    # position outright: a cap that is not announced reads as a complete answer.
    try:
        with opener.open(url, timeout=timeout) as response:
            body = response.read(MAX_BODY_BYTES + 1)
            row["status"] = response.status
            row["content_type"] = response.headers.get("Content-Type", "") or ""
            row["final_url"] = response.geturl()
    except urllib.error.HTTPError as exc:
        # An HTTP error is a RESULT, not a failure to probe: a 404 or a 451 is
        # exactly the fact the caller is asking for, so it gets a row.
        body = exc.read(MAX_BODY_BYTES + 1) if exc.fp else b""
        row["status"] = exc.code
        row["content_type"] = exc.headers.get("Content-Type", "") if exc.headers else ""
        row["final_url"] = url
    except (
        urllib.error.URLError,
        # `IncompleteRead` / `BadStatusLine` are raised during `.read()` and are
        # HTTPException, NOT OSError — without this they escape as a traceback
        # rather than the one clean stderr line this module promises.
        http.client.HTTPException,
        OSError,
        ValueError,
    ) as exc:
        body = b""
        row["error"] = str(exc)
    if len(body) > MAX_BODY_BYTES:
        body = body[:MAX_BODY_BYTES]
        row["truncated"] = True
    row["redirects"] = handler.count
    row["bytes"] = len(body)

    if body:
        path = os.path.join(out_dir, f"{label}.body")
        with open(path, "wb") as handle:
            handle.write(body)
    return row


#: A query string: `?` up to the next whitespace. Substituted rather than
#: truncated-at, so text AFTER the URL survives — an exception message is
#: usually "<reason>: <url> (<detail>)", and cutting at the first `?` would
#: throw away the detail that makes it a diagnosis.
_QUERY_RE = re.compile(r"\?\S*")


def redact_query(text: str) -> str:
    """``text`` with every query string in it replaced by ``?…``.

    The reported URL is the one place a probe can leak a credential into the
    transcript, and this repo's own feed venues authenticate **by query
    parameter** (Twelve Data and Alpha Vantage both take `apikey=`), so
    `--url av=https://…/query?…&apikey=SECRET` is an intended usage shape
    rather than a hypothetical. The tool has no notion of which parameter is a
    secret, so it reports none of them — the host and path are what a
    reachability probe is actually about.

    Takes arbitrary text, not just a URL, because the paths that need it
    include exception strings and argv echoes.
    """
    return _QUERY_RE.sub("?…", text)


def format_rows(rows: list[dict]) -> list[str]:
    """The compact table, one line per endpoint.

    A redirected or truncated endpoint is flagged **inline** rather than left to
    be inferred from a column the reader may skim past — the same reasoning in
    both cases: a fact the caller needs must be structural, not lucky.
    """
    lines = []
    for row in rows:
        if row["error"]:
            lines.append(f"{row['label']}  ERROR  {redact_query(row['error'])}")
            continue
        flag = f" REDIRECTED({row['redirects']})" if row["redirects"] else ""
        if row.get("truncated"):
            flag += f" TRUNCATED(at {MAX_BODY_BYTES}B)"
        lines.append(
            f"{row['label']}  {row['status']}  {row['bytes']}B  "
            f"{row['content_type'] or '-'}  {redact_query(row['final_url'])}{flag}"
        )
    return lines


def _read_url_file(path: str) -> list[str]:
    try:
        with open(path, encoding="utf-8") as handle:
            return [
                line.strip()
                for line in handle
                if line.strip() and not line.lstrip().startswith("#")
            ]
    except OSError as exc:
        raise ProbeError(f"could not read {path}: {exc}") from exc


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="probe_endpoints.py")
    parser.add_argument(
        "--out-dir", required=True, help="directory bodies are written to"
    )
    parser.add_argument(
        "--url",
        action="append",
        default=[],
        help="a label=url pair (repeatable)",
    )
    parser.add_argument("--url-file", help="a file of label=url lines")
    parser.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT)
    args = parser.parse_args(argv[1:])

    raw = list(args.url)
    if args.url_file:
        raw += _read_url_file(args.url_file)
    if not raw:
        raise ProbeError("no endpoints given — pass --url and/or --url-file")

    pairs = [parse_label_url(item) for item in raw]
    labels = [label for label, _ in pairs]
    # Case-INSENSITIVE, because the filesystem this protects usually is. On the
    # default macOS APFS/HFS+ volume `ecb` and `ECB` name the same file, so a
    # case-sensitive check passes and then one body silently overwrites the
    # other — precisely the failure this guard exists to prevent, and invisible
    # because both table rows still print correctly.
    if len({label.lower() for label in labels}) != len(labels):
        raise ProbeError(
            "labels must be unique (case-insensitively) — they name the body files"
        )

    os.makedirs(args.out_dir, exist_ok=True)
    rows = [probe_one(label, url, args.out_dir, args.timeout) for label, url in pairs]
    for line in format_rows(rows):
        print(line)
    redirected = sum(1 for row in rows if row["redirects"])
    summary = f"probe-endpoints | {len(rows)} probed, bodies in {args.out_dir}"
    if redirected:
        summary += (
            f" | {redirected} REDIRECTED — the feeds client refuses redirects, "
            f"so these are NOT reachable by it"
        )
    print(summary, file=sys.stderr)
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except ProbeError as exc:
        print(f"probe-endpoints: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
