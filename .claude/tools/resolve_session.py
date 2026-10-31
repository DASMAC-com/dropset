#!/usr/bin/env python3
"""Resolve which Claude Code session belongs to a worktree tag, and how to reach
it — the addressing `task resume` needs and `--continue` alone cannot supply.

**The bug this exists to fix.** `task resume <n>` used to `cd` into the `eng-<n>`
worktree and run `claude --continue`, on the reasonable assumption that
`--continue`'s per-directory addressing selects that worktree's session. It does
not, for a session started by `task`: `task` runs `claude -w <tag>` **from the base
repo**, so Claude Code files the transcript under the *base* repo's project slug
even though every one of its `cwd` stamps points into the worktree. No project
directory for the worktree ever exists, `--continue` finds nothing there, and
`task resume` reports "no conversation found" while the session sits intact under
another slug.

Measured: one session's transcript was recovered by hand at
``~/.claude/projects/<base-slug>/<uuid>.jsonl`` while its worktree was present
and its PR open. Sessions that had been resumed *from inside* their worktree at
some point did have a worktree-slug transcript, which masked the gap — so the
failure looks intermittent and tracks launch history rather than anything the
caller did.

`fleet` types `task resume`, so fleet resume inherits the same miss for every
`-w`-launched session that has never been resumed from inside its worktree.

**That filing rule has since changed.** Current Claude Code files a `-w`
session under the *worktree's* slug (measured 2026-10-13), which the
``continue`` mode below already reaches. Both layouts remain on disk, so both
paths stay; neither is dead code.

Usage::

    python3 .claude/tools/resolve_session.py --tag 1051

It prints one JSON object and never opens a session itself — deciding is this
tool's job, launching is the shell verb's. The worktree is the bare-number
directory, or a pre-rename ``eng-`` / ``ceo-`` / ``exp-`` one when only that
exists (see :func:`worktree_candidates`):

    {
      "tag": "1051",
      "worktree": "/…/.claude/worktrees/1051",
      "worktree_exists": true,
      "mode": "resume",
      "session_id": "afce0c54-1874-4276-a77b-38df146d0000",
      "run_from": "/…/dropset",
      "reason": "the worktree has no transcript of its own; …"
    }

``mode`` is the instruction to the caller:

* ``continue`` — the worktree has its own transcript; `cd` there and
  ``claude --continue``, the original fast path. ``session_id`` names the
  newest transcript, the one ``--continue`` reaches; the launcher's exit
  estimate reads it. Branch on ``mode``, never on whether the id is set.
* ``resume`` — a base-slug transcript's ``cwd`` stamps point into this
  worktree; run ``claude --resume <session_id>`` from ``run_from``.
* ``picker`` — nothing resolved deterministically; fall back to
  ``claude --resume <tag>``, which filters the picker rather than resuming.

Reading transcripts happens in **this** process, so a multi-megabyte JSONL never
reaches a transcript of its own (per ``docs/conventions/context-economy.md``).
Only the head of each candidate is read: `claude -w` stamps the cwd from the
first record, so scanning the whole file to find one would be pure cost.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_resolve_session.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
from datetime import datetime
from pathlib import Path

# How many leading records of a candidate transcript are scanned for a `cwd`
# stamp. `claude -w` sets the working directory before the first record is
# written, so the answer is in the head or it is not there at all — and these
# files run to hundreds of megabytes, which is the whole reason this scan is
# bounded rather than exhaustive.
CWD_SCAN_LINES = 200

# Keys a transcript record may carry the working directory under. `cwd` is what
# Claude Code writes today; the alternates cost nothing to accept and mean a
# format tweak degrades to the picker instead of to a wrong answer.
CWD_KEYS = ("cwd", "workingDirectory", "working_dir")


class ResolveSessionError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def claude_home() -> Path:
    """The Claude Code state directory — ``CLAUDE_CONFIG_DIR`` or ``~/.claude``.

    Mirrors ``prune_conversations.py`` rather than re-deciding it: two tools
    reading the same tree must agree on where it is.
    """
    configured = os.environ.get("CLAUDE_CONFIG_DIR")
    if configured:
        return Path(configured)
    home = os.environ.get("HOME")
    if not home:
        raise ResolveSessionError("neither CLAUDE_CONFIG_DIR nor HOME is set")
    return Path(home) / ".claude"


def slugify(path: Path) -> str:
    """Claude Code names each project's transcript dir after the working dir,
    replacing every ``/`` and ``.`` with ``-``. Same scheme as
    ``prune_conversations.py``, and the one
    ``docs/conventions/local-integrations.md`` points at for the rule.
    """
    return "".join("-" if c in "/." else c for c in str(path))


#: The prefixes an issue-keyed worktree was created under before the bare-number
#: rename — `eng-` for `task` and issue-keyed `explore`, `ceo-` for `architect`,
#: `exp-` for an older explore. Mirrors `_ds_worktree_of` in
#: `.claude/shell/init.zsh`.
LEGACY_PREFIXES = ("eng-", "ceo-", "exp-")

_ISSUE_TAG_RE = re.compile(r"\A(?:(?:eng|ceo|exp)-)?0*(\d+)\Z", re.ASCII)


def normalize_tag(raw: str) -> str:
    """``1051``, ``eng-1051``, ``ENG-1051`` and ``ceo-1051`` all name issue 1051,
    so all normalize to the bare ``1051`` — the name every issue-keyed session
    and worktree now carries. Anything else (a worded tag, which the launch
    verbs now refuse) passes through lowercased.

    Matches ``_ds_tag_of`` in the shell, so the tool and the verbs agree on what
    a tag is.
    """
    tag = raw.strip().lower()
    if not tag:
        raise ResolveSessionError("--tag was empty")
    # A path separator or a `..` segment would escape the worktrees directory
    # when joined below. `worktree.is_dir()` gates the traversal off in
    # practice, but rejecting it here keeps the tag a tag.
    if "/" in tag or os.sep in tag or tag in (".", "..") or ".." in tag.split("-"):
        raise ResolveSessionError(f"--tag must name a single worktree: {raw!r}")
    match = _ISSUE_TAG_RE.match(tag)
    return str(int(match.group(1))) if match else tag


def worktree_candidates(tag: str) -> list[str]:
    """The directory names that may hold ``tag``'s worktree, preferred first:
    the bare number, then each pre-rename spelling. Those are grandfathered,
    not moved — a live transcript stamps its cwd into the old path — so this is
    the seam that keeps an in-flight ``ceo-<n>`` thread reachable by number."""
    bare = normalize_tag(tag)
    if not (bare.isascii() and bare.isdigit()):
        return [bare]
    return [bare, *(p + bare for p in LEGACY_PREFIXES)]


def _mtime(path: Path) -> float:
    """``path``'s mtime, or 0.0 if it cannot be read.

    Guarded because `is_file()` / `is_dir()` swallow errors but `stat()` does
    not, and this tree is written concurrently by live sessions and
    hard-deleted from by ``prune_conversations.py --apply``. A file vanishing
    between the glob and the sort would otherwise raise an ``OSError`` that
    ``main`` does not catch — a raw traceback out of a tool whose whole error
    contract is a clean message and exit 2.
    """
    try:
        return path.stat().st_mtime
    except OSError:
        return 0.0


def transcripts_in(slug_dir: Path) -> list[Path]:
    """Top-level ``*.jsonl`` under one project slug, newest first.

    Top-level only, deliberately: a sub-agent transcript is not a session a
    human can resume, and offering one would be a wrong answer rather than a
    missing one.
    """
    if not slug_dir.is_dir():
        return []
    files = [p for p in slug_dir.glob("*.jsonl") if p.is_file()]
    return sorted(files, key=_mtime, reverse=True)


def stamps_into(transcript: Path, target: Path) -> bool:
    """True when this transcript's ``cwd`` stamps point at ``target``.

    This is the actual identification: a `-w`-launched session filed under the
    base slug (the older layout) says nothing by its slug, and the stamps say
    everything. A
    malformed or unreadable line is skipped rather than fatal — a transcript
    being written to right now can end mid-record, and that is the common case
    for a live session, which is exactly the one worth resuming.
    """
    target_str = str(target)
    try:
        with transcript.open(encoding="utf-8", errors="replace") as fh:
            for index, line in enumerate(fh):
                if index >= CWD_SCAN_LINES:
                    return False
                line = line.strip()
                if not line:
                    continue
                try:
                    record = json.loads(line)
                except (json.JSONDecodeError, ValueError):
                    continue
                if not isinstance(record, dict):
                    continue
                for key in CWD_KEYS:
                    value = record.get(key)
                    if not isinstance(value, str):
                        continue
                    normalized = os.path.normpath(value)
                    if normalized == target_str or normalized.startswith(
                        target_str + os.sep
                    ):
                        return True
    except OSError:
        return False
    return False


def resolve(tag: str, repo: Path) -> dict:
    """Decide how to reach the session for ``tag``. Pure filesystem inspection."""
    root = repo / ".claude" / "worktrees"
    names = worktree_candidates(tag)
    worktree = next((root / n for n in names if (root / n).is_dir()), root / names[0])
    verdict = {
        "tag": tag,
        "worktree": str(worktree),
        "worktree_exists": worktree.is_dir(),
        "mode": "picker",
        "session_id": None,
        "run_from": str(repo),
        "reason": "",
    }

    if not verdict["worktree_exists"]:
        # The pre-existing fallback, and still correct: the worktree was pruned
        # after a merge but the transcript outlives it.
        verdict["reason"] = (
            f"no worktree at {worktree} — it was likely pruned; the picker "
            f"filtered by tag is the only form that still reaches the session"
        )
        return verdict

    projects = claude_home() / "projects"

    # The fast path: a worktree with its own transcript is exactly what
    # `--continue` addresses — and, since Claude Code began filing `-w` sessions
    # under the worktree's slug, the usual case rather than the exception.
    own = transcripts_in(projects / slugify(worktree))
    if own:
        verdict["mode"] = "continue"
        # The newest transcript is the one `--continue` reaches, so after the
        # session exits it names the session the launcher's epilogue prices.
        verdict["session_id"] = own[0].stem
        verdict["run_from"] = str(worktree)
        verdict["reason"] = (
            f"{worktree.name} has its own transcript, so --continue addresses "
            f"it from inside the worktree"
        )
        return verdict

    # The miss this tool exists for. Check the base slug first — where older
    # Claude Code filed a `-w` session — then every other project dir, since a
    # session could have been launched from somewhere else entirely.
    base_slug = projects / slugify(repo)
    searched = [base_slug]
    others = (
        sorted(
            (p for p in projects.iterdir() if p.is_dir() and p != base_slug),
            key=_mtime,
            reverse=True,
        )
        if projects.is_dir()
        else []
    )
    searched.extend(others)

    for slug_dir in searched:
        for transcript in transcripts_in(slug_dir):
            if stamps_into(transcript, worktree):
                verdict["mode"] = "resume"
                verdict["session_id"] = transcript.stem
                verdict["run_from"] = str(repo)
                verdict["reason"] = (
                    f"the worktree has no transcript of its own, but "
                    f"{transcript.name} under {slug_dir.name} stamps its cwd "
                    f"into the worktree — resume it by id from the base repo"
                )
                return verdict

    verdict["reason"] = (
        f"the worktree exists but no transcript anywhere under {projects} "
        f"stamps its cwd into it — the session may never have started"
    )
    return verdict


#: The kinds `.claude/shell/init.zsh` actually seeds a daily id with — the
#: scheduler's `plan`, the curator's `curate` and the launcher's `launch` (the
#: `housekeeping` verb retired). Anything else has no session to name.
DAILY_KINDS = ("plan", "curate", "launch")

#: The daily kinds that cycle; `launch` always seeds cycle 0
#: (`_ds_daily_sid launch 0`), so no counter file can move it.
CYCLING_KINDS = ("plan", "curate")

#: The kinds seeded by issue number instead of by day (`_ds_topic_sid chore
#: <n>`), reached through ``--daily-id chore --tag <n>`` so a chore's tail has
#: the same one computed path a seat's does.
TOPIC_KINDS = ("chore",)

_DAILY_DATE_RE = re.compile(r"^\d{8}$")

#: Where the launcher keeps per-launch state — the substrate markers, and the
#: planning hub's cycle counter beside them. Mirrors `_DS_SUBSTRATE_DIR` in
#: `.claude/shell/init.zsh`, relative to the base checkout.
STATE_DIR = Path(".claude") / "session-substrate"


def cycle_counter_path(repo: Path, date: str, kind: str = "plan") -> Path:
    """The per-day file holding how many times today's daily `kind` has cycled.

    Per DAY, so a reopen later the same day resumes the latest cycle rather
    than cycle 0, and tomorrow starts from 0 with nothing to clean up. Per
    KIND, so the scheduler and the curator cycle independently.
    """
    return repo / STATE_DIR / f"{kind}-cycle-{date}"


def read_cycle(repo: Path, date: str, kind: str = "plan") -> int:
    """Today's cycle number; an absent or garbled counter reads as 0, the
    pre-cycling id, so a missing file can only name the day's first session.

    Digits only, trailing newlines stripped and nothing else — the same rule as
    the launcher's `$(<counter)` plus `<->` test, so the two can never derive
    different ids from one hand-edited file.
    """
    try:
        raw = cycle_counter_path(repo, date, kind).read_text().rstrip("\n")
    except OSError:
        return 0
    return int(raw) if raw.isascii() and raw.isdigit() else 0


def launch_date() -> str:
    """The date a daily session was LAUNCHED on, not today's.

    The `plan` and `curate` launchers export `DS_PLAN_DATE` into the client, so
    a session still running past midnight computes the id it was started under
    rather than the new day's.
    """
    env = os.environ.get("DS_PLAN_DATE", "")
    if _DAILY_DATE_RE.match(env):
        return env
    return datetime.now().strftime("%Y%m%d")


def daily_session_id(kind: str, date: str, cycle: int = 0) -> str:
    """The session id a daily verb (`plan`, `curate`) computes for itself.

    A seat session's id is **deterministic**, not discovered: an md5 of
    ``dropset-<kind>-<YYYYMMDD>`` formatted as a UUID. So a session wanting its
    own transcript — to run the metrics tool over it at close-out — computes the
    id rather than searching for it.

    That distinction is worth a subcommand because the search is expensive and
    looks reasonable: one scheduler session listed the Claude projects directory
    to find its own transcript id, at **≈6.0k** for that one call and ≈6.4k
    across five, making a bare directory listing its top hardening candidate by
    result size. The next bootstrap reproduced the same id by computation at
    near-zero cost.

    **The seed must match `_ds_daily_sid` in `.claude/shell/init.zsh` exactly**,
    since that is what actually names the session at launch. The full date is in
    it so `schedule-18` in August and `schedule-18` in September cannot collide — the
    display name is day-only by operator choice, and this id is what
    disambiguates. An `architect` session deliberately uses a *topic* seed with no
    date, because it is meant to be resumed days later.

    A **cycled** scheduler session (the `plan` skill's "Cycling") appends
    ``-c<cycle>`` to the seed: a fresh id under the same display name. Cycle 0
    keeps the bare seed, so every id computed before cycling existed still holds.

    **Both inputs are validated, because a wrong one is not detectable
    downstream.** Any string hashes to a well-formed UUID, so ``--daily-id Plan``
    or ``--date 2026-09-10`` prints something that looks exactly like an answer
    and names a session that does not exist. The caller then reads the missing
    transcript as "my transcript is gone" rather than "I asked the wrong
    question" — and the whole point of this function is to replace a *search*,
    which would at least have come back visibly empty. Refusing is the only
    signal available.
    """
    if kind not in DAILY_KINDS:
        raise ResolveSessionError(
            f"unknown daily kind {kind!r} — the launcher seeds only "
            f"{', '.join(DAILY_KINDS)} by day and {', '.join(TOPIC_KINDS)} by "
            f"issue number. Any string would hash to a well-formed id "
            f"naming no session, so this refuses rather than guessing. (An "
            f"`architect` session is seeded by topic, not by day, and has no "
            f"daily id.)"
        )
    if not _DAILY_DATE_RE.match(date):
        raise ResolveSessionError(
            f"--date must be YYYYMMDD with no separators, got {date!r} — the seed "
            f"is a literal `date +%Y%m%d`, so any other spelling hashes to the "
            f"wrong id."
        )
    seed = f"dropset-{kind}-{date}" + (f"-c{cycle}" if cycle > 0 else "")
    return _seed_uuid(seed)


def _seed_uuid(seed: str) -> str:
    digest = hashlib.md5(seed.encode("utf-8")).hexdigest()  # noqa: S324 - a naming seed, not a security primitive
    return (
        f"{digest[0:8]}-{digest[8:12]}-{digest[12:16]}-{digest[16:20]}-{digest[20:32]}"
    )


def topic_session_id(kind: str, tag: str) -> str:
    """The id a number-seeded verb (``chore <n>``) launched under — the seed
    ``dropset-<kind>-<n>`` of `_ds_topic_sid`, which must match it exactly.

    ``tag`` is the bare number or ``eng-<n>``; anything else refuses, for the
    reason ``daily_session_id`` gives: a wrong input still hashes to an id.
    """
    if kind not in TOPIC_KINDS:
        raise ResolveSessionError(
            f"unknown topic kind {kind!r} — only {', '.join(TOPIC_KINDS)} is "
            f"seeded by issue number"
        )
    number = normalize_tag(tag)
    if not number.isdigit():
        raise ResolveSessionError(
            f"--daily-id {kind} needs --tag <n>, an issue number, got {tag!r}"
        )
    return _seed_uuid(f"dropset-{kind}-{number}")


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="resolve_session.py",
        description="Resolve how to reach a worktree tag's Claude Code session.",
    )
    parser.add_argument(
        "--daily-id",
        default=None,
        metavar="KIND",
        help="print the deterministic session id for a daily verb (plan, "
        "curate, launch), or a chore with --tag <n>, and "
        "exit — compute your own id rather than listing the "
        "projects directory to find it",
    )
    parser.add_argument(
        "--date",
        default=None,
        metavar="YYYYMMDD",
        help="the date for --daily-id (default: today, local time, matching "
        "what the launcher used)",
    )
    parser.add_argument(
        "--cycle",
        type=int,
        default=None,
        metavar="N",
        help="the cycle for --daily-id (default: the launcher's counter for "
        "that kind and date under --repo, 0 when none — so run it from the "
        "base checkout)",
    )
    parser.add_argument("--tag", help="eng-### or a bare number")
    parser.add_argument(
        "--repo",
        default=None,
        help="base repo checkout (default: the CWD's repo root by convention)",
    )
    parser.add_argument(
        "--format",
        choices=("json", "lines"),
        default="json",
        help="json (default, for humans and jq) or lines — mode, session id "
        "and run-from directory on three lines, for a shell `read`",
    )
    args = parser.parse_args(argv[1:])

    # `abspath`, NOT `resolve()`. Claude Code derives the slug from the working
    # directory **string** it was given, so the slug has to be computed from
    # that same string — and `resolve()` follows symlinks, silently rewriting it.
    # macOS is where this bites: `/tmp` and `/var` are symlinks, so a resolved
    # path yields a slug for a directory that does not exist, and every lookup
    # misses while looking entirely correct. `abspath` normalizes a relative
    # argument without touching symlinks, which is exactly the needed half.
    repo = Path(os.path.abspath(args.repo if args.repo else Path.cwd()))

    if args.daily_id in TOPIC_KINDS:
        # Refused, not ignored: a day or cycle has no part in a number seed,
        # and silently dropping one hides a wrong question.
        if args.date or args.cycle is not None:
            raise ResolveSessionError(
                f"--daily-id {args.daily_id} is seeded by issue number alone; "
                f"--date and --cycle do not apply"
            )
        if not args.tag:
            raise ResolveSessionError(
                f"--daily-id {args.daily_id} needs --tag <n>, the issue number"
            )
        print(topic_session_id(args.daily_id, args.tag))
        return 0
    if args.daily_id:
        # `daily_session_id` refuses any kind outside DAILY_KINDS.
        date = args.date or launch_date()
        cycle = args.cycle
        # Validated like the kind and the date, and for the same reason: a
        # wrong value still hashes to a well-formed id naming no session.
        if cycle is not None and cycle < 0:
            raise ResolveSessionError(f"--cycle must be >= 0, got {cycle}")
        if cycle and args.daily_id not in CYCLING_KINDS:
            raise ResolveSessionError(
                f"--daily-id {args.daily_id} never cycles (the launcher seeds "
                f"cycle 0 only), got --cycle {cycle}"
            )
        if cycle is None:
            cycle = (
                read_cycle(repo, date, args.daily_id)
                if args.daily_id in CYCLING_KINDS
                else 0
            )
        print(daily_session_id(args.daily_id, date, cycle))
        return 0

    # `--tag` is required for the resolution path but not for `--daily-id`, so it
    # is validated here rather than by argparse.
    if not args.tag:
        raise ResolveSessionError("--tag is required (or pass --daily-id KIND)")

    verdict = resolve(normalize_tag(args.tag), repo)
    if args.format == "lines":
        # Three fixed lines in a fixed order, so a shell reads them positionally
        # with no JSON parsing. The session id is empty rather than absent when
        # there is none, which keeps the line count constant — a caller doing
        # three `read`s must not have its third read consume a missing second.
        print(verdict["mode"])
        print(verdict["session_id"] or "")
        print(verdict["run_from"])
    else:
        print(json.dumps(verdict, indent=2))
    # 0 when the caller has a deterministic action, 1 when it must fall back to
    # the picker — so the shell can branch on the exit status alone.
    return 0 if verdict["mode"] in ("continue", "resume") else 1


def main() -> int:
    try:
        return run(sys.argv)
    except ResolveSessionError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
