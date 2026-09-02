#!/usr/bin/env python3
# cspell:word pgpass
# cspell:word PGSERVICE
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

**A caveat on ``--direct``.** The connection string is passed as an argv element,
where it is readable in the process table for the life of the query. That is
acceptable for a developer-invoked localnet tool against a local Postgres — which
is what ``DROPSET_DB_URL`` documents — but if that variable is ever pointed at a
shared or remote instance carrying a real password, move to ``PGPASSWORD`` /
``~/.pgpass`` / ``PGSERVICE`` rather than reaching for ``--direct``.

Note also that ``--sql`` and ``--file`` reach ``psql``, which accepts backslash
meta-commands (``\\!`` shells out, ``\\o`` and ``\\copy`` touch files), and that
``-v`` substitution into a ``.sql`` file is textual. The values come from the
developer running the tool, who already has a shell, so this is not an escalation
— but the argv surface is strictly more powerful than "run a query", which is
worth knowing given the whole point is to collapse into one stable allow-rule.

**It connects as the read-only role and refuses the owner.** This tool is a
verification *reader*, so it logs in as ``dropset_ro`` and rejects a
``DROPSET_DB_URL`` that names the ``dropset`` owner. Defaulting to the owner
made the one-writer-per-table rule an honor system on the read path — nothing
but care stopped a verification query being run by a role that could also
write. A write belongs to the service that owns the table.

**When the running stack's migration state lags the branch under test, target a
disposable instance — never the live stack.** Spin a throwaway Postgres on a
spare port and migrate that. Migrating the running localnet database to suit a
branch mutates shared state that other sessions depend on, and an applied
migration is immutable, so the repair is manual surgery or a data-destroying
wipe. The failure this prevents is quieter than either: a verification run
against a database whose schema does not match the code being verified will
happily return a green answer to the wrong question.

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

# The READ-ONLY role, and the default — this tool is a verification reader.
#
# Migration 0002 creates `dropset_ro` and grants it `SELECT` on every table in
# `public` (plus a default-privileges grant, so tables added later are covered),
# which is why defaulting to it is safe on any migrated database rather than
# only where Grafana happens to be provisioned.
#
# Defaulting to the owner made the one-writer-per-table rule an honor system on
# the read path: nothing but care stopped a verification query from being run by
# a role that could also write. This makes it mechanical.
DEFAULT_USER = "dropset_ro"

# The owner role, named here only so it can be REFUSED. A write belongs to the
# service that owns the table, never to an ad-hoc verification query.
OWNER_USER = "dropset"

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


def _url_user(db_url: str) -> str | None:
    """The username in a ``postgres://user:pass@host/db`` URL, or ``None``.

    Parsed rather than pattern-matched so a password containing ``@`` cannot
    shift which side of the split the username lands on, and percent-decoded
    because the client library decodes URI components — without that, a
    percent-escaped spelling of the owner role reads as a different string here
    and connects as the owner there.
    """
    from urllib.parse import unquote, urlsplit

    try:
        user = urlsplit(db_url).username
    except ValueError:
        # Unparseable: report it as "no username", which is what the caller's
        # allowlist then refuses. This used to say the opposite — that a
        # malformed URL must never become a refusal — and that was true under
        # the old denylist, where `None` meant "not the owner, pass it through".
        # Inverting to an allowlist inverted this too: `None` is now refused.
        # That is the right direction (fail closed), but the refusal message
        # will say "no username" for a string that is simply malformed, so read
        # it as "this guard could not identify a role" rather than as a precise
        # diagnosis.
        return None
    return unquote(user) if user is not None else None


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
    # `is None`, not truthiness: `--sql ''` passes exactly one source, and the
    # old `bool(sql) == bool(file)` reported "pass exactly one" for it — a
    # misdiagnosis of an empty value as a missing one.
    if (sql is None) == (file is None):
        raise LocalnetPsqlError("pass exactly one of --sql or --file")
    if not (sql or file or "").strip():
        raise LocalnetPsqlError("--sql/--file was given an empty value")

    psql: list[str] = ["psql"]
    if not direct:
        psql += ["-U", DEFAULT_USER, "-d", DEFAULT_DB]
    elif not db_url:
        raise LocalnetPsqlError(
            "--direct needs DROPSET_DB_URL set to a connection string"
        )
    elif _url_user(db_url) != DEFAULT_USER:
        # An ALLOWLIST, not a denylist of one — `--direct` is the path that
        # could otherwise smuggle the owner back in, and naming the single role
        # to reject left two ways through. A URL with **no** userinfo
        # (`postgres://127.0.0.1:5432/dropset`) is a legitimate libpq string, and
        # psql is exec'd with the inherited environment, so libpq resolves the
        # role from PGUSER or the OS account — which on the localnet container is
        # the superuser. Requiring the reader role by name closes that, and the
        # percent-decode above closes the other.
        #
        # It fails CLOSED: an unusual-but-harmless URL is refused rather than
        # quietly connecting as something this tool does not vouch for.
        named = _url_user(db_url)
        saw = f"as {named!r}" if named else "with no username"
        raise LocalnetPsqlError(
            f"DROPSET_DB_URL connects {saw}; this tool is a verification reader "
            f"and connects only as {DEFAULT_USER!r}. Name that role in the URL "
            f"— a write belongs to the service that owns the table, and an "
            f"unnamed user falls back to PGUSER or the OS account."
        )

    # The caller's own `--var` pairs go FIRST, so the fixed flags below win.
    # psql honors the LAST `-v` for a given name, so with this order reversed a
    # `--var ON_ERROR_STOP=0` silently disabled the tool's own error-stop guard.
    for pair in variables:
        if "=" not in pair:
            raise LocalnetPsqlError(f"--var needs name=value, got {pair!r}")
        psql += ["-v", pair]

    # `-P pager=off` matters even non-interactively: a configured pager in the
    # image would otherwise wait for input and the call would hang rather than
    # answer.
    psql += ["-P", "pager=off", "-v", "ON_ERROR_STOP=1"]
    if not aligned:
        psql += ["-A", "-F", FIELD_SEPARATOR]
    if tuples_only:
        psql.append("-t")

    if sql:
        psql += ["-c", sql]
    else:
        psql += ["-f", file]

    # The connection string goes LAST, after every flag. As a leading positional
    # it relied on getopt argument permutation, which `POSIXLY_CORRECT` in the
    # environment disables — psql would then read the flags as connection
    # parameters. Trailing is unconditionally correct.
    if direct:
        psql.append(db_url)
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
        # `--count` implies `-t`. Without it psql emits a column header and a
        # `(N rows)` footer, so counting non-blank output lines answered "how
        # many rows" with N+2 — wrong by two on the one mode whose entire job is
        # that number.
        tuples_only=args.tuples_only or args.count,
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
        # `-t` is implied above, so these are data rows: no header, no footer.
        rows = len([ln for ln in proc.stdout.splitlines() if ln.strip()])
        print(f"{rows} row(s)")
        return 0

    text, dropped = _cap(proc.stdout, args.max_rows)
    if text:
        print(text)
    if dropped:
        # Announced, never silent: a trimmed result that looks complete is how a
        # wrong conclusion gets drawn from a right query.
        # "line(s)", not "row(s)": the cap counts OUTPUT LINES, so the header and
        # the `(N rows)` footer are included and a value containing a newline
        # splits across several. `--count` uses the same wording for the same
        # reason. Note the footer is among what gets cut, so the authoritative
        # row count is exactly what a truncated read loses.
        print(
            f"-- {dropped} more output line(s) NOT shown, including psql's own "
            "row-count footer (raise --max-rows, or narrow the query)",
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
