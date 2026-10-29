#!/usr/bin/env python3
# cspell:word strerror
"""Report the cargo pool's lanes, and on ``--apply`` wipe every idle one.

The pool (``_ds_cargo_pool`` in ``.claude/shell/init.zsh``) points every dropset
checkout's cargo at one of ``DS_CARGO_LANES`` directories under the cargo target
root — ``dropset`` for lane 0, ``dropset-<i>`` for lane ``i``. Nothing else ever
removes from them, and they grow without bound (see
``docs/conventions/local-integrations.md`` → "The cargo pool"), so the daily
upkeep pass runs this tool and the wipe is the bound.

Usage::

    python3 .claude/tools/cargo_lanes.py [--apply] [--json]

Every entry under the target root is listed with its size: each lane (one not
created yet reads ``absent``), then every **non-lane** entry — the stale
two-hex-character directories of an older layout, a lane beyond the current
count, a stray per-issue directory, a tombstone a crashed wipe left behind.

**Busy is read from the build queue.** The cargo roots of the process table
(``machine_load.cargo_roots``, the probe's own read) are each attributed to the
entry their ``--target-dir`` or inherited ``CARGO_TARGET_DIR`` names, read from
``ps -E`` — a stopped build included, since it still holds its lane. An entry
with a root on it is skipped, and the line names the pid. A root whose
environment cannot be read, or that builds into the target root itself, makes
**every** entry busy — the probe fails closed, as does a process table that
cannot be read at all. Busy means a cargo process: a binary run straight out of
a lane with no cargo alive is not seen, which is safe for the Make targets
since they copy what they run into the worktree's own ``target/``.

**The wipe.** Without ``--apply`` nothing is touched (a dry run, as every prune
step is). With it, every idle entry is first renamed to a ``.wipe-`` tombstone —
all of them right after one probe, so a build starting mid-wipe meets a fresh
directory rather than one vanishing under it, and a numbered one when a crashed
wipe's tombstone already holds the name — and the tombstones are then removed
(a stray file or symlink is unlinked). A tombstone that survives a crash is a non-lane entry the next run
removes.

Stdout is a ranked summary; ``--json`` the full manifest. Exit 0 on a report,
2 on a usage error. Stdlib only; a Python skill-tool under ``.claude/tools/`` —
deliberately not a Cargo workspace member. Tests live in
``tests/test_cargo_lanes.py``.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Callable

import machine_load

LANE_PREFIX = "dropset"
TOMBSTONE = ".wipe-"
TIMEOUT_DU = 600

# (cmd) -> (rc, stdout); a missing binary or a timeout raises OSError.
Runner = Callable[[list[str]], "tuple[int, str]"]


def real_run(cmd: list[str]) -> tuple[int, str]:
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            errors="replace",
            check=False,
            timeout=TIMEOUT_DU,
        )
    except subprocess.TimeoutExpired as exc:
        raise OSError(f"{cmd[0]} timed out") from exc
    return proc.returncode, proc.stdout


def target_root(env: dict[str, str]) -> Path:
    """The directory the lanes live in, as the hook spells it."""
    home = env.get("CARGO_HOME") or str(
        Path(env.get("HOME", "~")).expanduser() / ".cargo"
    )
    return Path(home) / "target"


def lane_count(env: dict[str, str]) -> int:
    """``DS_CARGO_LANES`` as the hook reads it: 2 when unset, 1 when it is not
    a whole number of at least 1."""
    raw = env.get("DS_CARGO_LANES")
    if raw is None or raw == "":
        return 2
    return int(raw) if raw.isdecimal() and int(raw) >= 1 else 1


def lane_names(count: int) -> list[str]:
    return [LANE_PREFIX] + [f"{LANE_PREFIX}-{i}" for i in range(1, count)]


# --- the build probe ------------------------------------------------------

_TARGET_ARG = re.compile(r"(?:^|\s)--target-dir(?:=|\s+)(\S+)")
_TARGET_ENV = re.compile(r"(?:^|\s)CARGO_TARGET_DIR=(\S+)")
# Every process a login shell starts carries these; a `ps -E` line with
# neither is one whose environment ps did not show.
_ANY_ENV = re.compile(r"\s(?:PATH|HOME)=")
# `entry_of`'s answer for a build into the target root itself, which every
# entry then holds for.
WHOLE_ROOT = "*"


class ProbeFailed(Exception):
    """The build queue could not be read; every entry is then held."""


def target_of(command: str) -> str | None:
    """The target directory one ``ps -E`` command line builds into, or None for
    a cargo building its own checkout's ``target/``. An argument beats the
    environment, as it does for cargo. A line showing no environment at all
    raises, since its target cannot be known."""
    match = _TARGET_ARG.search(command) or _TARGET_ENV.search(command)
    if match:
        return match.group(1)
    if not _ANY_ENV.search(command):
        raise ProbeFailed("a cargo root's environment is unreadable")
    return None


def entry_of(target: str, root: Path) -> str | None:
    """The target root's entry a target directory sits in, if any — compared
    as spelled first (a symlinked lane resolves outside the root), then with
    both sides resolved (a symlinked cargo home). ``WHOLE_ROOT`` for the root
    itself."""
    for t, r in (
        (os.path.normpath(target), os.path.normpath(root)),
        (os.path.realpath(target), os.path.realpath(root)),
    ):
        try:
            rel = Path(t).relative_to(r)
        except ValueError:
            continue
        return rel.parts[0] if rel.parts else WHOLE_ROOT
    return None


def cargo_roots(table: list[machine_load.Proc]) -> list[machine_load.Proc]:
    """The probe's cargo roots, stopped ones included: a suspended build still
    holds its lane, though it waits on nothing the load probe counts."""
    by_pid = {p.pid: p for p in table}
    return [
        p
        for p in table
        if machine_load.is_cargo(p.comm)
        and not (p.ppid in by_pid and machine_load.is_cargo(by_pid[p.ppid].comm))
    ]


def _commands(run: Runner, pids: list[int]) -> dict[int, str]:
    rc, out = run(
        ["ps", "-E", "-ww", "-o", "pid=,command=", "-p", ",".join(map(str, pids))]
    )
    seen = machine_load.parse_args_ps(out)
    if rc != 0 and not seen:
        # A root that exited between the two reads makes ps exit 1 and can print
        # nothing; ask each pid alone, so one exit cannot hide a live build.
        for pid in pids:
            _, out = run(["ps", "-E", "-ww", "-o", "pid=,command=", "-p", str(pid)])
            seen.update(machine_load.parse_args_ps(out))
    return seen


def probe(
    run: Runner, root: Path, alive: Callable[[int], bool]
) -> dict[str, list[int]]:
    """``{entry: [pid, …]}`` for every target-root entry a cargo root is
    building on right now."""
    try:
        rc, out = run(machine_load.PS_TABLE)
        if rc != 0:
            raise ProbeFailed(f"ps exited {rc}")
        roots = cargo_roots(machine_load.parse_ps(out))
    except (OSError, machine_load.NoReading) as exc:
        raise ProbeFailed(str(exc)) from exc
    if not roots:
        return {}
    pids = [p.pid for p in roots]
    try:
        commands = _commands(run, pids)
    except OSError as exc:
        raise ProbeFailed(str(exc)) from exc
    busy: dict[str, list[int]] = {}
    for pid in pids:
        if pid not in commands:
            if alive(pid):
                raise ProbeFailed(f"cargo pid {pid} is running but unreadable")
            continue
        target = target_of(commands[pid])
        name = entry_of(target, root) if target else None
        if name == WHOLE_ROOT:
            raise ProbeFailed(f"cargo pid {pid} builds into the target root itself")
        if name:
            busy.setdefault(name, []).append(pid)
    return busy


def pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


# --- the manifest ---------------------------------------------------------


@dataclass
class Entry:
    name: str
    lane: bool
    bytes: int | None = None
    present: bool = True
    busy: list[int] = field(default_factory=list)
    action: str = ""


def size_of(run: Runner, path: Path) -> int | None:
    try:
        rc, out = run(["du", "-sk", str(path)])
    except OSError:
        return None
    head = out.split(None, 1)[:1]
    if rc != 0 or not head or not head[0].isdecimal():
        return None
    return int(head[0]) * 1024


def human(n: int | None) -> str:
    if n is None:
        return "size unread"
    if n < 1024**3:
        return f"{n / 1024**2:.1f} MiB"
    return f"{n / 1024**3:.1f} GiB"


def scan(root: Path, count: int, run: Runner) -> list[Entry]:
    lanes = lane_names(count)
    present = sorted(p.name for p in root.iterdir()) if root.is_dir() else []
    entries = [Entry(name, True, present=name in present) for name in lanes]
    entries += [Entry(name, False) for name in present if name not in lanes]
    for e in entries:
        if e.present:
            e.bytes = size_of(run, root / e.name)
    return entries


def wipe(
    root: Path,
    entries: list[Entry],
    busy: dict[str, list[int]] | None,
    apply: bool,
    remove: Callable[[Path], None] = shutil.rmtree,
) -> int:
    """Settle every entry's action; on ``apply`` rename each idle one to a
    tombstone, then remove the tombstones. Returns the bytes freed (or that a
    dry run would free)."""
    freed = 0
    doomed: list[tuple[Entry, Path]] = []
    for e in entries:
        if not e.present:
            e.action = "absent"
            continue
        if busy is None:
            e.action = "held: build probe failed"
            continue
        e.busy = busy.get(e.name, [])
        if e.busy:
            pids = ", ".join(map(str, e.busy))
            e.action = f"skipped: cargo build running (pid {pids})"
            continue
        if not apply:
            e.action = "would remove"
            freed += e.bytes or 0
            continue
        path = root / e.name
        if not e.name.startswith(TOMBSTONE):
            # A crashed wipe's tombstone may already hold the plain name.
            tomb = root / f"{TOMBSTONE}{e.name}"
            n = 1
            while tomb.exists() or tomb.is_symlink():
                tomb = root / f"{TOMBSTONE}{e.name}-{n}"
                n += 1
            try:
                path.rename(tomb)
            except OSError as exc:
                e.action = f"failed: {exc.strerror or exc}"
                continue
            path = tomb
        doomed.append((e, path))
    for e, path in doomed:
        try:
            # A stray file or symlink is unlinked; rmtree refuses both.
            if path.is_dir() and not path.is_symlink():
                remove(path)
            else:
                path.unlink()
        except OSError as exc:
            e.action = f"failed mid-removal: {exc.strerror or exc}"
            continue
        e.action = "removed"
        freed += e.bytes or 0
    return freed


def report(
    env: dict[str, str],
    apply: bool,
    run: Runner = real_run,
    alive: Callable[[int], bool] = pid_alive,
    remove: Callable[[Path], None] = shutil.rmtree,
) -> dict:
    root = target_root(env)
    count = lane_count(env)
    entries = scan(root, count, run)
    # Probed after the sizing, which is the slow part, so the renames follow
    # the read by milliseconds rather than by the length of a `du` walk.
    try:
        busy: dict[str, list[int]] | None = probe(run, root, alive)
        probe_line = "ok"
    except ProbeFailed as exc:
        busy, probe_line = None, f"failed: {exc}"
    freed = wipe(root, entries, busy, apply, remove)
    return {
        "root": str(root),
        "lanes": count,
        "dry_run": not apply,
        "probe": probe_line,
        "freed": freed,
        "entries": [asdict(e) for e in entries],
    }


def summary(result: dict) -> str:
    """One line: what was (or would be) removed, and what was held."""
    entries = result["entries"]
    verb = "would free" if result["dry_run"] else "freed"
    gone = [e for e in entries if e["action"] in ("removed", "would remove")]
    failed = [e for e in entries if e["action"].startswith("failed")]
    held = [e for e in entries if e["present"] and e not in gone and e not in failed]
    lanes = sum(e["lane"] for e in gone)
    line = (
        f"{verb} {human(result['freed'])}: {lanes} lane(s) and "
        f"{len(gone) - lanes} non-lane entries; {len(held)} held"
    )
    if failed:
        line += f", {len(failed)} failed"
    if result["probe"] != "ok":
        line += f" (build probe {result['probe']})"
    return line


def rows(result: dict) -> list[str]:
    """Each lane, then each held non-lane entry, then one row for the non-lane
    entries that went (or would go), naming the largest few."""

    def row(e: dict) -> str:
        if not e["present"]:
            return f"{e['name']} absent"
        return f"{e['name']} {human(e['bytes'])} — {e['action']}"

    gone = ("removed", "would remove")
    lanes = [row(e) for e in result["entries"] if e["lane"]]
    other = [e for e in result["entries"] if not e["lane"]]
    held = [row(e) for e in other if e["action"] not in gone]
    swept = sorted(
        (e for e in other if e["action"] in gone), key=lambda e: -(e["bytes"] or 0)
    )
    if not swept:
        return lanes + held
    largest = ", ".join(e["name"] for e in swept[:3])
    total = sum(e["bytes"] or 0 for e in swept)
    verb = "would remove" if result["dry_run"] else "removed"
    return (
        lanes
        + held
        + [
            f"{len(swept)} non-lane entries {human(total)} — {verb} (largest: {largest})"
        ]
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="cargo_lanes.py", description=__doc__.split("\n")[0]
    )
    parser.add_argument("--apply", action="store_true", help="remove every idle entry")
    parser.add_argument("--json", action="store_true", help="print the full manifest")
    args = parser.parse_args(argv)
    result = report(dict(os.environ), args.apply)
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(
            "\n".join(
                [
                    f"cargo lanes under {result['root']}: {summary(result)}",
                    *rows(result),
                ]
            )
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
