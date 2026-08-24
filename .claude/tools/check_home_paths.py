#!/usr/bin/env python3
"""Committed-agent-config hygiene: no file names a **real machine's** home
directory.

Two committed test modules referenced the operator's actual home path in their
fixtures. That violates the refer-to-users-in-the-abstract rule for committed
agent material (``CLAUDE.md`` → "Docs and skills prose"), and it makes a test
read machine-specific when it should be fixture-driven. This tool is the guard
so it cannot recur silently.

**What it does not do: forbid home paths outright.** The sibling tools are full
of them, correctly — ``allowlist.py``'s own examples, the worktree guard's
fixtures, and half of ``test_allowlist.py`` all need a path *shaped* like
``/Users/<someone>/...`` to say anything useful. Those use **placeholder**
segments (``/Users/me``, ``/Users/x``, ``/Users/nobody``), which is exactly the
right form. So the check is on the **user segment**, not on the shape: a
known placeholder passes, anything else fails.

**Why a placeholder allowlist rather than comparing against ``$HOME``.**
Comparing to the running user's home would be the obvious rule, and it is the
wrong one: it only catches the violation on the machine that committed it. CI runs
as some other user, so a real home path committed on a laptop would sail through
CI forever — which is precisely the review stage that should catch it. Matching
on the segment instead makes the check machine-independent, so it holds
identically on a laptop and on a runner.

Scope is set by the caller (pre-commit's ``files:`` regex), not here, so the
same tool can be pointed at a different tree without an edit. The decks are
exempt by operator ruling: naming a team member in presentation *content* is
fine, and it is only a committed test hardcoding their machine that is not.

Stdlib only. This is a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import re
import sys
from typing import NamedTuple

# A POSIX home-directory prefix plus its user segment. `/Users/` is macOS and
# `/home/` is Linux; both appear in this repo's material. The segment stops at
# the next separator, so `/Users/me/repos/x` yields `me`.
_HOME_PATH_RE = re.compile(r"/(?:Users|home)/([A-Za-z0-9_.\-]+)")

# User segments that read as a stand-in rather than as somebody's account. Kept
# deliberately short: the point is that a writer reaching for one of these has
# *chosen* a placeholder, and a longer list starts accepting real short
# usernames by accident. `you` and `user` are here for prose, the rest for
# fixtures.
PLACEHOLDERS = frozenset(
    {
        "a",
        "me",
        "nobody",
        "someone",
        "user",
        "x",
        "you",
    }
)


class Finding(NamedTuple):
    """One offending occurrence."""

    path: str
    line_no: int
    segment: str
    text: str

    def render(self) -> str:
        return f"{self.path}:{self.line_no}: /…/{self.segment}/ — {self.text.strip()}"


def offending_segments(line: str) -> list[str]:
    """Return the non-placeholder user segments named in ``line``.

    Every match is checked, not just the first: one line can name two paths,
    and a placeholder earlier in it must not excuse a real one later.
    """
    return [
        segment
        for segment in _HOME_PATH_RE.findall(line)
        if segment not in PLACEHOLDERS
    ]


def scan_text(path: str, text: str) -> list[Finding]:
    """Scan one file's contents, returning a Finding per offending occurrence."""
    findings: list[Finding] = []
    for line_no, line in enumerate(text.splitlines(), start=1):
        findings.extend(
            Finding(path, line_no, segment, line)
            for segment in offending_segments(line)
        )
    return findings


def scan_files(paths: list[str]) -> list[Finding]:
    """Scan each path, skipping anything that isn't decodable text.

    A binary file under a scanned tree (a committed ``.wasm``, say) is not what
    this guard is about, and a ``UnicodeDecodeError`` escaping here would fail
    the hook for a reason that has nothing to do with the rule. A path that is
    gone (staged then deleted) is likewise not a violation.

    The skip list is deliberately **narrow**. An earlier version caught
    ``OSError`` wholesale, which additionally swallowed a permission error and
    a bad file descriptor — cases where the guard returns "pass" for a file it
    never read. For a check whose whole value is having no hole, an unreadable
    file should fail loudly rather than silently count as clean.
    """
    findings: list[Finding] = []
    for path in paths:
        try:
            with open(path, encoding="utf-8") as handle:
                text = handle.read()
        except (FileNotFoundError, IsADirectoryError, UnicodeDecodeError):
            continue
        findings.extend(scan_text(path, text))
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="check_home_paths.py",
        description=(
            "Fail if a committed file names a real machine's home directory. "
            "A placeholder user segment (" + ", ".join(sorted(PLACEHOLDERS)) + ") "
            "passes; anything else does not."
        ),
    )
    parser.add_argument("paths", nargs="*", help="files to scan")
    args = parser.parse_args(argv)

    findings = scan_files(args.paths)
    if not findings:
        return 0

    for finding in findings:
        print(finding.render(), file=sys.stderr)
    print(
        f"\ncheck-home-paths: {len(findings)} real home path(s) in committed "
        "agent material. Use a placeholder segment ("
        + ", ".join(sorted(PLACEHOLDERS))
        + ") or a tmp-path fixture — never a real account name.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
