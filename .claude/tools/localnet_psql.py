#!/usr/bin/env python3
# cspell:word screenful
"""Query the localnet Postgres in one allow-rule, without psql's box drawing.

SQL verification against the localnet database used to go through hand-written
``docker exec … psql …`` invocations — one measured session ran twenty of them
(~3.3k) and paid a **fresh permission prompt per variant**, because every change
of flag order or query text is a different command line. That is the
prompt-churn shape ``CLAUDE.md`` → "Shell commands" exists to collapse: one
stable command prefix, only the arguments varying.

The second saving is the output. ``psql`` defaults to aligned output, which draws
box borders and pads every column to its widest value — pure formatting bytes
replayed on every later turn. This defaults to unaligned output with an explicit
separator, so a three-column answer costs three columns. Pass ``--aligned`` when
a human is going to read the table directly.

Usage::

    python3 .claude/tools/localnet_psql.py --sql 'select count(*) from candles'
    python3 .claude/tools/localnet_psql.py --file market-data/analytics/q.sql \\
        --var source=coinbase --var product_id=EURC-USDC
    python3 .claude/tools/localnet_psql.py --sql 'select 1' --count

Connection: by default it execs into the localnet container
(``dropset-localnet-postgres-1``, overridable with
``DROPSET_LOCALNET_PG_CONTAINER``). With ``--direct`` it runs a local ``psql``
against ``DROPSET_DB_URL`` instead, which is what the analytics README documents
for a non-container Postgres.

Row output is capped and the cap is **announced** — a silent truncation reads as
a complete answer, which is the one thing worse than a verbose one. Stdlib only;
a Python skill-tool under ``.claude/tools/`` — deliberately **not** a Cargo
workspace member. Tests live in ``tests/test_localnet_psql.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys

# The localnet compose project's Postgres service container.
DEFAULT_CONTAINER = "dropset-localnet-postgres-1"

DEFAULT_USER = "dropset"
DEFAULT_DB = "dropset"

# Column separator for unaligned output. " | " reads like the aligned form
# without paying for the padding.
FIELD_SEPARATOR = " | "

# Rows printed before the output is cut. A verification query answers a question;
# past a screenful it is a data dump, and the answer to that is a narrower query.
DEFAULT_MAX_ROWS = 50

# Overall timeout, so a lock-blocked query cannot wedge a session.
DEFAULT_TIMEOUT = 60


class LocalnetPsqlError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def build_argv(
    *,
    sql: str | None,
    file: str | None,
    variables: list[str],
    direct: bool,
    aligned: bool,
    tuples_only: bool,
    container: str,
    db_url: str | None,
) -> list[str]:
    """The full command to run, as an exec'able argv list.

    Pure, so the flag assembly is testable without a database or a container.
    """
    if bool(sql) == bool(file):
        raise LocalnetPsqlError("pass exactly one of --sql or --file")

    psql: list[str] = ["psql"]
    if direct:
        if not db_url:
            raise LocalnetPsqlError(
                "--direct needs DROPSET_DB_URL set to a connection string"
            )
        psql.append(db_url)
    else:
        psql += ["-U", DEFAULT_USER, "-d", DEFAULT_DB]

    # `-P pager=off` matters even non-interactively: a configured pager in the
    # image would otherwise wait for input and the call would hang rather than
    # answer.
    psql += ["-P", "pager=off", "-v", "ON_ERROR_STOP=1"]
    if not aligned:
        psql += ["-A", "-F", FIELD_SEPARATOR]
    if tuples_only:
        psql.append("-t")

    for pair in variables:
        if "=" not in pair:
            raise LocalnetPsqlError(f"--var needs name=value, got {pair!r}")
        psql += ["-v", pair]

    if sql:
        psql += ["-c", sql]
    else:
        psql += ["-f", file]

    if direct:
        return psql
    return ["docker", "exec", "-i", container, *psql]


def _cap(text: str, max_rows: int) -> tuple[str, int]:
    """The output trimmed to ``max_rows`` lines, plus how many were dropped."""
    lines = text.splitlines()
    if max_rows <= 0 or len(lines) <= max_rows:
        return text.rstrip("\n"), 0
    return "\n".join(lines[:max_rows]), len(lines) - max_rows


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="localnet_psql.py")
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--sql", default=None, help="a single statement to run")
    source.add_argument("--file", default=None, help="a .sql file to run")
    parser.add_argument(
        "--var",
        action="append",
        default=[],
        metavar="NAME=VALUE",
        help="a psql variable (repeatable)",
    )
    parser.add_argument(
        "--direct",
        action="store_true",
        help="run a local psql against DROPSET_DB_URL instead of docker exec",
    )
    parser.add_argument(
        "--aligned",
        action="store_true",
        help="keep psql's aligned table output (box borders and padding)",
    )
    parser.add_argument(
        "--tuples-only",
        action="store_true",
        help="drop the header and row-count footer",
    )
    parser.add_argument(
        "--count",
        action="store_true",
        help="report only how many rows came back, not the rows",
    )
    parser.add_argument(
        "--max-rows",
        type=int,
        default=DEFAULT_MAX_ROWS,
        help=f"rows before the output is cut (default {DEFAULT_MAX_ROWS}; 0 = all)",
    )
    parser.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT)
    args = parser.parse_args(argv[1:])

    command = build_argv(
        sql=args.sql,
        file=args.file,
        variables=args.var,
        direct=args.direct,
        aligned=args.aligned,
        tuples_only=args.tuples_only,
        container=os.environ.get(
            "DROPSET_LOCALNET_PG_CONTAINER", DEFAULT_CONTAINER
        ).strip()
        or DEFAULT_CONTAINER,
        db_url=os.environ.get("DROPSET_DB_URL", "").strip() or None,
    )

    try:
        proc = subprocess.run(
            command,
            capture_output=True,
            text=True,
            check=False,
            timeout=args.timeout,
        )
    except FileNotFoundError as e:
        raise LocalnetPsqlError(f"cannot run {command[0]}: {e}") from e
    except subprocess.TimeoutExpired:
        raise LocalnetPsqlError(
            f"query did not finish within {args.timeout}s — a lock, or a query "
            "that wants narrowing"
        ) from None

    if proc.returncode != 0:
        # The stderr tail is the whole value of a failed run, so it is passed
        # through rather than summarized.
        detail = (proc.stderr or proc.stdout or "").strip()
        raise LocalnetPsqlError(f"psql exited {proc.returncode}: {detail}")

    if args.count:
        rows = len([ln for ln in proc.stdout.splitlines() if ln.strip()])
        print(f"{rows} row line(s)")
        return 0

    text, dropped = _cap(proc.stdout, args.max_rows)
    if text:
        print(text)
    if dropped:
        # Announced, never silent: a trimmed result that looks complete is how a
        # wrong conclusion gets drawn from a right query.
        print(
            f"-- {dropped} more row(s) NOT shown (raise --max-rows, or narrow "
            "the query)",
            file=sys.stderr,
        )
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except LocalnetPsqlError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
