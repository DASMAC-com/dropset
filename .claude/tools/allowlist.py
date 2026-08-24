#!/usr/bin/env python3
"""``settings.local.json`` allowlist parser — the shared, context-cheap reader
for the ``permissions.allow`` array that both ``firm-perms`` and
``housekeeping`` step 7 need, without either whole-reading the ~250-entry file
into the model's context (per ``CLAUDE.md`` → "Context economy" / "Skill
tooling").

Three subcommands. All three print JSON to stdout; ``covers`` and ``cruft``
only read the settings file, while ``add`` **writes** it (and deliberately does
not read it first — that is the whole point). ``--settings PATH`` is a top-level
option, so it precedes the subcommand
(``allowlist.py --settings PATH covers RULE``):

* ``covers RULE`` — is ``RULE`` already granted by the
  allowlist (exactly, or subsumed by a broader existing rule)? Prints
  ``{covered, insertion_index, would_subsume, count}`` — ``insertion_index`` is
  where an uncovered rule would append (end of the array), and
  ``would_subsume`` lists the indices of existing narrower entries the new rule
  would make redundant. The membership + subsumption logic is ``firm_core``'s,
  so it matches what ``firm_last.py`` writes.
* ``add RULE`` — the **write** counterpart of ``covers``, closing the loop so a
  hand-firm never has to read the allowlist at all. ``covers`` already computes
  where the rule would land; ``add`` performs that append (via
  ``firm_core.firm_into``, the same writer the fast firm uses, so subsumed narrower
  entries are pruned in the same pass) and prints
  ``{rule, added, covered, refused, count}``. This exists because ``Edit``
  requires a prior ``Read`` of the file it edits: firming one Bash rule by hand
  cost a whole-file ``Read`` of a 338-entry ``settings.local.json``, which is
  precisely what this module was written to prevent. ``add`` is idempotent — an
  already-covered rule reports ``added: false`` and leaves the file untouched —
  and it enforces the **safety floor**, refusing any rule
  ``_over_broad_reason`` would flag (a bare wildcard, a bare-verb wildcard, an
  unscoped file-access root) with a non-zero exit rather than granting it.
* ``cruft`` — return only the **suspicious** entries
  (``{index, rule, category, reason}``) plus the total ``count``, so the audit
  reasons over a short shortlist instead of the whole array. Categories mirror
  ``housekeeping`` step 7: ``over-broad`` (a bare-verb wildcard or an unscoped
  file-access root), ``subsumed`` (a narrower rule an earlier one already
  covers — the dead weight ``firm-perms`` never prunes), ``dangerous`` (an
  ``rm -rf`` / force-push / pipe-to-shell one-off), ``machine-path`` (a
  malformed path, or an absolute home path in a settings file where one does
  not belong), ``machine-path-stale`` (a path that no longer resolves on
  disk — the shape worktree rules decay into as worktrees are pruned), and
  ``guard-conflict`` (a rule granting a command shape a committed
  ``PreToolUse`` guard blocks outright).

  **``guard-conflict`` is the semantic pass.** Every other category is a
  *pattern* judgment about the rule's own text; this one asks whether the rule
  contradicts a **stated convention that is mechanically enforced elsewhere**,
  which no amount of pattern-matching on the rule alone can see. The live case:
  the allowlist carried
  ``Bash(git -C /…/worktrees/* grep:*)`` while ``CLAUDE.md`` and
  ``local-integrations.md`` say the git-grep guard has **no escape hatch** and
  is "kept absolute on purpose". The entry is inert — the guard blocks at the
  ``PreToolUse`` layer regardless of what the allowlist permits, and a guard
  block is not a permission question — but it is *misleading*, reading as
  sanction for a practice the conventions forbid, and it would become live
  again if the guard were ever unwired.

  It asks the **guard's own predicate**, imported from
  ``.claude/hooks/``, rather than re-deriving "what counts as ``git grep``"
  here. Two copies of that judgment would drift, and the guard's version is
  the one that actually decides at run time. If the guard cannot be imported
  the check reports nothing rather than raising: an audit tool must not break
  because a hook moved.

  **``machine-path`` is file-aware.** In a ``settings.local.json`` — git-ignored
  and machine-local by design — an absolute home path is correct, not drift, so
  it is not flagged there merely for being absolute. Flagging it unconditionally
  produced 39 false positives out of 40 on one real pass, nearly all of them
  load-bearing worktree and skill-tooling rules. The response carries
  ``machine_local_settings`` so a reader knows which rule was in force.

Defaults ``--settings`` to ``.claude/settings.local.json`` in the cwd, and
**resolves that default through a worktree to the main checkout** when the cwd
has no such file. Claude Code itself resolves the file that way, so a worktree
legitimately has none of its own — see ``docs/conventions/local-integrations.md``
→ "How settings files resolve across worktrees". A missing file is therefore
the *normal* case for a worktree, not a usage error, and reads treat it as an
empty allowlist. Stdlib only; a Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling").
"""

from __future__ import annotations

import argparse
import functools
import importlib.util
import json
import re
import sys
from pathlib import Path

import firm_core

DEFAULT_SETTINGS = ".claude/settings.local.json"

# The committed guards live one directory over, and are not importable as a
# package (`.claude` is not a Python root), so they are loaded by path.
_HOOKS_DIR = Path(__file__).resolve().parent.parent / "hooks"

# Absolute home paths, machine-specific by nature.
#
# Flagging these unconditionally made the shortlist unusable: on one pass
# `cruft` returned **40 flagged entries out of 359, and 39 were false
# positives**, every one of them `machine-path`. The file being audited is
# `.claude/settings.local.json`, which is git-ignored and machine-local *by
# design* — so an absolute `/Users/<name>/…` is the correct and only possible
# form there, not drift. Worse, the flagged set was dominated by load-bearing
# rules: the `git -C <base>/.claude/worktrees/*` entries the worktree workflow
# requires, the `~/.zshrc` reads the local-integrations doc tells you to make,
# and the `python3 <base>/.claude/tools/*` skill-tooling entry point. Removing
# any of them breaks the workflow it serves, so a human had to reject nearly
# the whole list by hand — which is the work the check was meant to remove.
#
# So an absolute home path is flagged only in a settings file where it does not
# belong. The shapes below are defects in *any* file and stay unconditional.
_MACHINE_PATH_RE = re.compile(r"/(Users|home)/[^/*]+/")

# A doubled slash can never match — this is the one true positive that pass
# found (`Read(//Users/<name>/.cargo/**)`), and it is malformed regardless of
# which settings file it sits in. The lookbehind exempts a URL scheme
# (`https://`, `file://`) and nothing else: a doubled slash *mid*-path is just
# as unmatchable as a leading one, so it must not be exempted too.
_MALFORMED_PATH_RE = re.compile(r"(?<!:)//")

# A path prefix worth resolving on disk. Anchored at the rule's first absolute
# path and stopped before any glob, so `Read(/Users/x/repo/**)` resolves
# `/Users/x/repo`. Worktree rules accumulate as worktrees come and go, so a
# no-longer-resolving path is the version of this check with real value.
#
# `:` is excluded from the path body because in a `Bash(...)` rule it is the
# argument separator, not part of the filename — capturing it turned
# `Bash(python3 /abs/tool.py:*)` into a lookup for `/abs/tool.py:`, which never
# resolves, so every absolute Bash rule would have read as stale.
_ABS_PATH_RE = re.compile(r"(/(?:Users|home)/[^/*\s:]+/[^*\s():]*)")

# Machine-local settings files: absolute home paths are expected here.
_LOCAL_SETTINGS_NAMES = ("settings.local.json",)

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


# Re-exported so callers and tests have one name for it; the implementation
# lives in firm_core because firm_last.py needs the identical answer, and two
# separately-written copies of it drifted.
find_main_checkout = firm_core.main_checkout


def resolve_settings_path(path: Path | None, *, explicit: bool = False) -> Path:
    """Where the settings file actually lives.

    Claude Code resolves ``.claude/settings.local.json`` through a worktree to
    the **main checkout**, so a worktree has no copy of its own. When the
    caller did not name a path, the main checkout's copy is used
    **unconditionally** — not only when it already exists.

    That unconditional-ness is the point. Keying the fallback on
    ``resolved.exists()`` looked safer but inverted the fix: on a main checkout
    with no allowlist yet, ``add`` would scaffold into the *worktree* instead —
    a file nothing ever reads, which then exists forever and shadows the real
    one on every later call. ``firm_last`` refuses outright in that situation;
    this now agrees with it by writing where the file belongs.

    An ``explicit`` ``--settings`` path is **never** redirected, even when it
    does not exist. A caller that names a file means that file: silently
    retargeting it would send an ``add`` write to a different allowlist than
    the one asked for, and would leave a caller-supplied path impossible to
    scaffold.
    """
    if explicit:
        if path is None:
            raise AllowlistError("an explicit --settings path cannot be empty")
        return path
    resolved = firm_core.main_settings_path()
    if resolved is not None:
        return resolved
    # No main checkout resolvable (git unavailable, or nothing on `main`).
    # Fall back to the cwd-relative default so a read still has something to
    # look at, rather than erroring on a path we cannot compute.
    return path if path is not None else Path(DEFAULT_SETTINGS)


def load_allow(path: Path) -> list[str]:
    """The ``permissions.allow`` array from a settings file.

    A **missing** file yields an empty list, not an error: under the shared-file
    model a worktree legitimately has no copy of its own, so absence is the
    normal case rather than a bad path. (Callers that need the resolved
    location should run the path through :func:`resolve_settings_path` first.)
    An unreadable or malformed file still raises ``AllowlistError`` — that is a
    real defect, and silently treating a corrupt file as empty would hide it.
    A well-formed file with no ``permissions.allow`` array yields an empty list.
    """
    try:
        settings = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return []
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


def add(rule: str, path: Path) -> dict:
    """Append ``rule`` to ``path``'s allow array unless already covered.

    Delegates the write to ``firm_core.firm_into`` — the writer the fast firm uses,
    so subsumed narrower entries are pruned identically — and neither path needs
    the allowlist in context. Re-reads the array afterwards only to report the
    new ``count``; the array itself never leaves this process.

    **The safety floor is enforced here, not in the writer.** ``firm_into`` has
    no floor of its own — ``firm_last.py`` checks ``is_bareverb_wildcard`` in its
    *caller* and returns before writing. So a write path that called
    ``firm_into`` directly would grant exactly what the fast firm refuses, and because
    this tool runs under the pre-approved directory-wide
    ``Bash(python3 .claude/tools/:*)`` rule, that would be a single
    non-prompting call that widens the agent's own Bash grant to a whole
    hazardous verb. ``add`` therefore refuses anything ``_over_broad_reason``
    flags — the same classifier ``cruft`` reports with, so this tool can never
    write what its own audit mode would immediately flag — returning
    ``added: false`` with a ``refused`` reason instead.
    """
    over_broad = _over_broad_reason(rule)
    if over_broad is not None:
        return {
            "rule": rule,
            "added": False,
            "covered": False,
            "refused": f"{over_broad} — narrow it by hand instead of firming",
            "count": len(load_allow(path)) if path.exists() else 0,
        }
    added = firm_core.firm_into(path, rule)
    allow = load_allow(path)
    return {
        "rule": rule,
        "added": added,
        "covered": not added,
        "refused": None,
        "count": len(allow),
    }


def _unscoped_file_root(rule: str) -> bool:
    m = _RULE_RE.match(rule.strip())
    if m is None or m.group(1) not in _FILE_TOOLS:
        return False
    return bool(_UNSCOPED_ROOT_RE.match(m.group(2).strip()))


def _over_broad_reason(rule: str) -> str | None:
    """The reason ``rule`` is over-broad, or ``None`` if it isn't."""
    if rule.strip() in ("Bash(:*)", "Bash( *)", "Bash(*)"):
        return "bare Bash wildcard — grants every command"
    if firm_core.is_bareverb_wildcard(rule):
        return "bare-verb wildcard — grants the whole program"
    if _unscoped_file_root(rule):
        return "unscoped file-access root"
    return None


def _is_subsumed(index: int, allow: list[str]) -> bool:
    """Whether ``allow[index]`` is dead weight another entry already covers.
    Checks the **whole** list, not just earlier entries — ``firm-perms``
    *appends* generalized rules, so the common layout is a narrow rule with the
    broader one that subsumes it sitting *after* it. A **strictly broader**
    coverer flags the narrow rule regardless of position; an **exact-equivalent**
    duplicate flags only the later copy (so one survives). A coverer that is
    itself **over-broad** is skipped — it's flagged for removal on its own, so
    the entries under it aren't the dead weight to report."""
    rule = allow[index]
    for j, other in enumerate(allow):
        if j == index or _over_broad_reason(other) is not None:
            continue
        if not firm_core.is_covered(rule, [other]):
            continue
        if not firm_core.is_covered(other, [rule]):
            return True  # `other` is strictly broader
        if j < index:
            return True  # exact-equivalent duplicate — keep the earlier one
    return False


@functools.lru_cache(maxsize=None)
def _load_guard(module_name: str):
    """Import a committed ``PreToolUse`` guard by path, or ``None``.

    Returns ``None`` on **any** failure. This is an *audit* tool: it must
    report what it can rather than refuse to run because a hook was moved,
    renamed, or edited into something that no longer imports — and a guard it
    cannot load simply contributes no findings.

    ``except Exception`` is deliberate rather than a list of expected classes.
    An earlier version caught ``(OSError, SyntaxError, ImportError,
    ValueError)``, which covers a *file* rename but not a module whose
    top-level execution raises anything else — so the promise in the paragraph
    above was not actually kept, and one renamed symbol could take down the
    whole ``cruft`` run. The narrow catch was the bug; breadth is the contract.

    Cached: ``classify`` runs per rule, so an uncached load compiles and
    executes the guard once for every entry — 387 times on the real allowlist.
    """
    path = _HOOKS_DIR / f"{module_name}.py"
    try:
        spec = importlib.util.spec_from_file_location(module_name, path)
        if spec is None or spec.loader is None:
            return None
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    except Exception:
        return None
    return module


def _rule_command(rule: str) -> str | None:
    """The command shape a ``Bash(...)`` rule grants, or ``None``.

    An allow-rule's inner text is a command **prefix** with a trailing wildcard
    (``git -C /x grep:*``), so the suffix is stripped to leave something a
    command-shape predicate can tokenize. Any ``*`` further left is left alone:
    it stands for a path segment, and the guards tokenize on whitespace, so it
    reads as an ordinary argument.
    """
    match = _RULE_RE.match(rule.strip())
    if match is None or match.group(1) != "Bash":
        return None
    inner = match.group(2).strip()
    for suffix in (":*", " *"):
        if inner.endswith(suffix):
            inner = inner[: -len(suffix)]
            break
    return inner.strip() or None


def guard_conflict(rule: str) -> str | None:
    """The reason a committed guard blocks what ``rule`` grants, else ``None``.

    Asks the guard's **own** predicate rather than re-deriving its rule here,
    so the audit and the run-time block cannot drift apart.

    **Exactly one guard is consulted today**, and the boundary is worth stating
    rather than leaving as an accident of implementation:

    * ``no_compound_bash`` is excluded **by policy** — it takes a
      ``#compound-ok`` marker, so a rule granting a compound is not in conflict
      with it. The marker is the sanctioned way through, and flagging those
      would bury the real finding in noise.
    * ``worktree_edit_guard`` is excluded **by construction** — it governs
      file-mutating tools, and :func:`_rule_command` returns ``None`` for any
      non-``Bash`` rule, so a ``Read(...)`` / ``Edit(...)`` grant can never
      reach a guard check at all.

    The predicate is fetched with ``getattr`` rather than attribute access: a
    guard that loads but has had its predicate renamed would otherwise raise
    ``AttributeError`` from inside ``classify`` and take down the whole
    ``cruft`` run — the same class of event ``_load_guard``'s catch exists for,
    arriving one step later.
    """
    command = _rule_command(rule)
    if command is None:
        return None
    guard = _load_guard("no_git_grep")
    predicate = getattr(guard, "is_git_grep", None) if guard else None
    if callable(predicate) and predicate(command):
        return (
            "grants `git grep`, which the no_git_grep guard blocks outright "
            "(no escape hatch, by design) — inert while the guard is wired, "
            "but it reads as sanctioning a practice the conventions forbid"
        )
    return None


def is_machine_local_settings(settings_path: Path | None) -> bool:
    """Whether absolute home paths are *expected* in this settings file."""
    if settings_path is None:
        return False
    return settings_path.name in _LOCAL_SETTINGS_NAMES


def stale_path(rule: str) -> str | None:
    """The first absolute path in ``rule`` that no longer resolves, if any."""
    match = _ABS_PATH_RE.search(rule)
    if not match:
        return None
    candidate = match.group(1).rstrip("/")
    if not candidate:
        return None
    return None if Path(candidate).exists() else candidate


def classify(
    rule: str,
    index: int,
    allow: list[str],
    machine_local: bool = False,
) -> tuple[str, str] | None:
    """Classify ``allow[index]`` as cruft, or ``None`` if it looks fine.

    ``allow`` is the whole array (``index`` names the entry) so the subsumed
    check can see broader rules on either side of it. ``machine_local`` says the
    settings file is one where absolute home paths belong, which suppresses the
    bare ``machine-path`` verdict without suppressing the malformed and stale
    checks — those are defects in any file.
    """
    over_broad = _over_broad_reason(rule)
    if over_broad is not None:
        return "over-broad", over_broad
    for reason, pattern in _DANGEROUS_RES:
        if pattern.search(rule):
            return "dangerous", reason
    # Before the path checks: a guard conflict is about what the rule *grants*,
    # and it would otherwise be masked by the `machine-path` verdict on the
    # absolute worktree path the live instance happens to carry.
    guarded = guard_conflict(rule)
    if guarded is not None:
        return "guard-conflict", guarded
    if _MALFORMED_PATH_RE.search(rule):
        return "machine-path", "doubled slash in the path — this rule can never match"
    if _MACHINE_PATH_RE.search(rule):
        if not machine_local:
            # In a shared settings file the absolute path is *itself* the
            # defect, so report that rather than whether it happens to resolve.
            return "machine-path", "absolute home path pinned into the rule"
        missing = stale_path(rule)
        if missing is not None:
            # Where absolute paths are legitimate, staleness is the real signal
            # — worktree rules accumulate as worktrees come and go.
            return "machine-path-stale", f"path no longer exists on disk: {missing}"
    if _is_subsumed(index, allow):
        return "subsumed", "another rule already covers this"
    return None


def cruft(allow: list[str], settings_path: Path | None = None) -> dict:
    """The suspicious-entry shortlist, keeping the full array out of context."""
    machine_local = is_machine_local_settings(settings_path)
    flagged = []
    for i, rule in enumerate(allow):
        verdict = classify(rule, i, allow, machine_local=machine_local)
        if verdict is not None:
            category, reason = verdict
            flagged.append(
                {"index": i, "rule": rule, "category": category, "reason": reason}
            )
    return {
        "count": len(allow),
        "flagged": flagged,
        # Stated so a reader knows why absolute paths went unflagged.
        "machine_local_settings": machine_local,
    }


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="allowlist.py")
    parser.add_argument(
        "--settings",
        # A `None` sentinel, NOT the default string: value-equality against
        # DEFAULT_SETTINGS cannot tell "not passed" from "passed the literal
        # default", so a caller who typed the default path explicitly would be
        # silently redirected — contradicting resolve_settings_path's contract
        # that an explicit path is never redirected.
        default=None,
        help=f"path to the settings file (default: {DEFAULT_SETTINGS} at the "
        "main checkout)",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_covers = sub.add_parser("covers", help="is a candidate rule already granted?")
    p_covers.add_argument("rule", help="the candidate allow-rule to test")

    p_add = sub.add_parser("add", help="append a rule (no prior read needed)")
    p_add.add_argument("rule", help="the allow-rule to add")

    sub.add_parser("cruft", help="return only the suspicious entries")

    args = parser.parse_args(argv[1:])
    # Resolve the default to the main checkout — Claude Code resolves the file
    # that way, so a worktree legitimately has no copy of its own. An
    # explicitly passed --settings is honored verbatim.
    settings_path = resolve_settings_path(
        Path(args.settings) if args.settings is not None else None,
        explicit=args.settings is not None,
    )

    if args.cmd == "add":
        result = add(args.rule, settings_path)
    elif args.cmd == "covers":
        result = covers(args.rule, load_allow(settings_path))
    else:
        result = cruft(load_allow(settings_path), settings_path)

    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    # A refused write exits non-zero so a caller that only checks the status
    # can't mistake "the floor rejected this rule" for "the rule was granted".
    if result.get("refused"):
        return 1
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except (AllowlistError, firm_core.SettingsError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
