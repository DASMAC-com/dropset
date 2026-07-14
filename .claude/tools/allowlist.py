#!/usr/bin/env python3
"""``settings.local.json`` allowlist parser — the shared, context-cheap reader
for the ``permissions.allow`` array that both ``firm-perms`` and
``housekeeping`` step 7 need, without either whole-reading the ~250-entry file
into the model's context (per ``CLAUDE.md`` → "Context economy" / "Skill
tooling").

Two subcommands, each reading a settings file and printing JSON to stdout:

* ``covers RULE [--settings PATH]`` — is ``RULE`` already granted by the
  allowlist (exactly, or subsumed by a broader existing rule)? Prints
  ``{covered, insertion_index, would_subsume, count}`` — ``insertion_index`` is
  where an uncovered rule would append (end of the array), and
  ``would_subsume`` lists the indices of existing narrower entries the new rule
  would make redundant. The membership + subsumption logic is ``firm_core``'s,
  so it matches what ``firm_last.py`` writes.
* ``cruft [--settings PATH]`` — return only the **suspicious** entries
  (``{index, rule, category, reason}``) plus the total ``count``, so the audit
  reasons over a short shortlist instead of the whole array. Categories mirror
  ``housekeeping`` step 7: ``over-broad`` (a bare-verb wildcard or an unscoped
  file-access root), ``subsumed`` (a narrower rule an earlier one already
  covers — the dead weight ``firm-perms`` never prunes), ``dangerous`` (an
  ``rm -rf`` / force-push / pipe-to-shell one-off), and ``machine-path`` (an
  absolute home path that leaked into a rule).

Defaults ``--settings`` to ``.claude/settings.local.json`` in the cwd. Stdlib
only; a Python skill-tool under ``.claude/tools/`` — deliberately **not** a
Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

import firm_core

DEFAULT_SETTINGS = ".claude/settings.local.json"

# Absolute home paths are machine-specific and shouldn't be pinned into a rule.
_MACHINE_PATH_RE = re.compile(r"/(Users|home)/[^/*]+/")

# Dangerous one-off shapes: destructive rm, force-push, pipe-to-shell installs.
_DANGEROUS_RES = (
    ("rm -rf / -r -f one-off", re.compile(r"\brm\b.*-\w*r\w*f|\brm\b.*-\w*f\w*r")),
    ("force push", re.compile(r"push.*--force(?!-with-lease)")),
    ("pipe to shell", re.compile(r"(curl|wget).*\|\s*(sudo\s+)?(sh|bash|zsh)")),
)

# File-access tools whose inner path, if it's a bare root wildcard, grants far
# too much (``Read(/**)``, ``Edit(**)``).
_FILE_TOOLS = ("Read", "Edit", "Write", "NotebookEdit")
_UNSCOPED_ROOT_RE = re.compile(r"^/?\*{1,2}/?$")
_RULE_RE = re.compile(r"^([A-Za-z_]\w*)\((.*)\)$", re.DOTALL)


class AllowlistError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def load_allow(path: Path) -> list[str]:
    """The ``permissions.allow`` array from a settings file. A missing or
    malformed file yields an empty list (nothing to check / audit)."""
    try:
        settings = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise AllowlistError(f"no settings file at {path}") from exc
    except (OSError, ValueError) as exc:
        raise AllowlistError(f"cannot parse {path}: {exc}") from exc
    if not isinstance(settings, dict):
        return []
    allow = settings.get("permissions", {}).get("allow")
    return [r for r in allow if isinstance(r, str)] if isinstance(allow, list) else []


def covers(rule: str, allow: list[str]) -> dict:
    """Whether ``rule`` is already covered, where an uncovered rule would append,
    and which existing entries it would subsume (be broader than)."""
    covered = firm_core.is_covered(rule, allow)
    would_subsume = [
        i for i, existing in enumerate(allow) if firm_core.is_covered(existing, [rule])
    ]
    return {
        "rule": rule,
        "covered": covered,
        "insertion_index": len(allow),
        "would_subsume": would_subsume,
        "count": len(allow),
    }


def _unscoped_file_root(rule: str) -> bool:
    m = _RULE_RE.match(rule.strip())
    if m is None or m.group(1) not in _FILE_TOOLS:
        return False
    return bool(_UNSCOPED_ROOT_RE.match(m.group(2).strip()))


def classify(rule: str, earlier: list[str]) -> tuple[str, str] | None:
    """Classify one allow entry as cruft, or ``None`` if it looks fine.
    ``earlier`` is the entries before it (so a duplicate is flagged once, on the
    later copy, not mutually)."""
    if rule.strip() in ("Bash(:*)", "Bash( *)", "Bash(*)"):
        return "over-broad", "bare Bash wildcard — grants every command"
    if firm_core.is_bareverb_wildcard(rule):
        return "over-broad", "bare-verb wildcard — grants the whole program"
    if _unscoped_file_root(rule):
        return "over-broad", "unscoped file-access root"
    for reason, pattern in _DANGEROUS_RES:
        if pattern.search(rule):
            return "dangerous", reason
    if _MACHINE_PATH_RE.search(rule):
        return "machine-path", "absolute home path pinned into the rule"
    if firm_core.is_covered(rule, earlier):
        return "subsumed", "an earlier rule already covers this"
    return None


def cruft(allow: list[str]) -> dict:
    """The suspicious-entry shortlist, keeping the full array out of context."""
    flagged = []
    for i, rule in enumerate(allow):
        verdict = classify(rule, allow[:i])
        if verdict is not None:
            category, reason = verdict
            flagged.append(
                {"index": i, "rule": rule, "category": category, "reason": reason}
            )
    return {"count": len(allow), "flagged": flagged}


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="allowlist.py")
    parser.add_argument(
        "--settings",
        default=DEFAULT_SETTINGS,
        help=f"path to the settings file (default {DEFAULT_SETTINGS})",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_covers = sub.add_parser("covers", help="is a candidate rule already granted?")
    p_covers.add_argument("rule", help="the candidate allow-rule to test")

    sub.add_parser("cruft", help="return only the suspicious entries")

    args = parser.parse_args(argv[1:])
    allow = load_allow(Path(args.settings))

    if args.cmd == "covers":
        result = covers(args.rule, allow)
    else:
        result = cruft(allow)

    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except AllowlistError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
