#!/usr/bin/env python3
"""Put a commit message file into the skip-by-default shape, in place.

Every session commit carries the ``skip-checks: true`` trailer, so its push
starts no ``pull_request`` workflow and the PR's required checks stay pending
until ``ci_runs.py trigger`` makes the one commit without it (``docs/ci.md`` →
"8. Local-first remote runs"). GitHub reads the trailer only after **two**
empty lines, and git's default cleanup collapses consecutive blank lines, so
a ``-m subject -m "skip-checks: true"`` commit started all five workflows on
PR 771 while the same message committed with ``--cleanup=verbatim -F <file>``
started none. This tool writes that file's shape; the commit paths then run::

    python3 .claude/tools/commit_message.py <file>
    git commit -S --cleanup=verbatim -F <file>

The rewrite strips trailing whitespace and the blank lines around the
message, collapses blank-line runs in the body to one (the part of default
cleanup that ``--cleanup=verbatim`` skips; ``#`` lines are kept, so a message
file carries none), drops any ``skip-checks``
line already present, and appends two empty lines and ``skip-checks: true``
as the last line, after any other trailer. It is idempotent, so a retried
commit can re-run it on the same file.

Prints one line starting ``commit-message |`` and exits 0; exits 2 on a
missing file or a message with no subject.

Standard library only; not a Cargo workspace member (``CLAUDE.md`` → "Skill
tooling"). Tests: ``tests/test_commit_message.py``, via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

PREFIX = "commit-message |"

#: The trailer every session commit ends with.
SKIP_TRAILER = "skip-checks: true"

_SKIP_LINE = re.compile(r"^skip-checks\s*:", re.IGNORECASE)


def with_skip_trailer(message: str) -> str:
    """Return ``message`` with the trailer after two empty lines, last.

    Raises ``ValueError`` when no subject line remains.
    """
    lines = [
        line.rstrip() for line in message.splitlines() if not _SKIP_LINE.match(line)
    ]
    body: list[str] = []
    for line in lines:
        if line or (body and body[-1]):
            body.append(line)
    while body and not body[-1]:
        body.pop()
    if not body:
        raise ValueError("the message has no subject line")
    return "\n".join(body) + "\n\n\n" + SKIP_TRAILER + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("file", help="the commit message file, rewritten in place")
    args = parser.parse_args(argv)
    path = Path(args.file)
    try:
        text = path.read_text(encoding="utf-8")
        path.write_text(with_skip_trailer(text), encoding="utf-8")
    except (OSError, ValueError) as exc:
        print(f"{PREFIX} {path}: {exc}", file=sys.stderr)
        return 2
    print(f"{PREFIX} {path}: {SKIP_TRAILER} after two empty lines, last")
    return 0


if __name__ == "__main__":
    sys.exit(main())
