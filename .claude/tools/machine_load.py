#!/usr/bin/env python3
# cspell:word loadavg
# cspell:word ncpu
# cspell:word swapusage
# cspell:word memorystatus
"""Read the machine and say how many more sessions of a class fit.

The fleet ceiling used to be a board literal; what actually refuses a launch
is this machine — the shared cargo pool lock every building session queues
on, memory, load, swap. One bare call reads all of it::

    python3 .claude/tools/machine_load.py
    python3 .claude/tools/machine_load.py --sessions 3 --class building
    python3 .claude/tools/machine_load.py --json
    python3 .claude/tools/machine_load.py --sample
    python3 .claude/tools/machine_load.py --json --sample --record <path>

``--record`` appends one compact JSON line to a session's launch record,
``sessions/<id>.load`` (the launcher's, on every create and every resume, so
resumes accumulate): the time, the class, the build queue, memory in use and
compressed, ``load1``, the building and free session counts, the lane count
(``DS_CARGO_LANES``, the pool's default 2 when unset; null when not a
number) and the verdict for the class. A launch
with no reading still appends a ``no-reading`` line. The digest's wait
section reads them (``wait_ledger.py``).

The reading is three commands and one targeted read, no sampling loop:
``vm_stat``; ``sysctl -n vm.loadavg hw.ncpu hw.memsize vm.swapusage
kern.memorystatus_vm_pressure_level``;
``ps -axc -o pid=,ppid=,rss=,etime=,stat=,comm=`` (``-c``: comm is the
executable's own name); and one ``ps -ww -o pid=,args= -p <roots>`` over the
cargo roots only. Never a pattern over every
process — ``pgrep -f claude`` matches every shell whose command line names a
``.claude`` path.

**Build queue.** A cargo invocation is the topmost process of a cargo chain
(``cargo``, ``cargo-*``; ``cargo-clippy`` runs a child ``cargo``, which is the
same invocation). It is *building* when anything that is not cargo runs under
it — ``rustc`` / ``rustdoc``, a test binary, the linker — and *waiting* when it
holds no such descendant: a cargo blocked on the pool lock has spent under a
second of CPU, so its age is its wait. A stopped cargo (ps state ``T``) is
neither, and ``cargo-watch`` is a watcher, not an invocation: the cargo it
spawns is the root. Each is named by its step from its args (``clippy``,
``doc``, ``test``, ``nextest``, ``build``, ``check``, ``run``). One snapshot:
a build caught in the instant between two children reads as waiting, an
error toward refusing that the next probe heals.

**Memory in use** is anonymous + wired + compressor-occupied pages times the
page size — Activity Monitor's "Memory Used" (a shade over: it keeps the
pages the system may discard on demand), so the operator can check the tool against what they already
look at. File cache and free pages are headroom.

**Sessions by class** come from the launch records in the session-substrate
directory: a ``<key>.class`` file beside a substrate marker holds the class
and the session's ``claude`` pid, ``free 12345``. A record counts only while
its pid is a live ``claude``, so a record a dead session left behind can
neither hide a building session nor invent a free one; every live ``claude``
without a ``free`` record counts as building, which is all of them until the
launcher writes records.

**One ceiling, every class.** ``DS_FLEET_TOTAL`` bounds the live ``claude``
processes — top-level clients, since a sub-agent runs inside its parent's
process — so a free seat and a building one spend the same number. A building
session is further refused on the pool-lock wait, and seated one at a time
while any build waits.

**Knobs** come from the environment (the operator's untracked runtime config);
the repo carries no machine literal. Two are **required**, with no default:
``DS_FLEET_TOTAL`` and ``DS_FLEET_CPU_LOAD_MAX`` (the one-minute load average,
read as cores in use, above which nothing new starts). An unset or malformed
one is a no reading naming the variable. Two have defaults:
``DS_FLEET_BUILD_WAIT_S`` (120) and ``DS_FLEET_RESERVE_GB`` (physical / 8).
Swap is reported, never a refusal: macOS leaves a stale residue paged out
long after the pressure that caused it has gone.

Exit 0 green (every asked session fits), 1 amber (fewer fit), 2 red (none),
3 no reading (not Darwin, a command or parse failure, a bad argument or knob,
any unexpected error — never 1 or 2, which a caller would read as a verdict).
A caller never halts on 3: it falls back to the total alone and says so.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_machine_load.py``.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import platform
import re
import subprocess
import sys
from collections import Counter
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

GIB = 1024**3

#: Mirrors `_DS_SUBSTRATE_DIR` in `.claude/shell/init.zsh`, relative to the
#: base checkout (one directory shared by every worktree).
STATE_DIR = Path(".claude") / "session-substrate"
WORKTREE_SEGMENT = "/.claude/worktrees/"
SAMPLE_LOG = "fleet-load.log"
# The cargo pool's lane count when `DS_CARGO_LANES` is unset — the default
# `_ds_cargo_pool` in `.claude/shell/init.zsh` applies.
DEFAULT_LANES = 2

STEPS = ("clippy", "doc", "test", "nextest", "build", "check", "run")
COMPILERS = ("rustc", "rustdoc", "clippy-driver")
#: Cargo-family names that are not a build invocation of their own.
NOT_ROOTS = ("cargo-watch",)
DOCKER_VM = "com.apple.Virtualization.VirtualMachine"
PRESSURE = {1: "normal", 2: "warn", 4: "critical"}

SYSCTL = [
    "sysctl",
    "-n",
    "vm.loadavg",
    "hw.ncpu",
    "hw.memsize",
    "vm.swapusage",
    "kern.memorystatus_vm_pressure_level",
]
PS_TABLE = ["ps", "-axc", "-o", "pid=,ppid=,rss=,etime=,stat=,comm="]

GREEN, AMBER, RED, NO_READING = 0, 1, 2, 3
COLOUR = {GREEN: "GREEN", AMBER: "AMBER", RED: "RED"}


class NoReading(Exception):
    """The machine could not be read; the caller falls back to the total."""


Runner = Callable[[list[str]], str]


def run(cmd: list[str]) -> str:
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            errors="replace",
            check=False,
            timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise NoReading(f"{cmd[0]}: {exc}") from exc
    if proc.returncode != 0:
        raise NoReading(f"{cmd[0]} exited {proc.returncode}")
    return proc.stdout


# --- parsing -------------------------------------------------------------


def parse_vm_stat(text: str) -> dict[str, int]:
    """Page counts by `vm_stat` label, plus ``page_size`` in bytes."""
    size = re.search(r"page size of (\d+) bytes", text)
    if not size:
        raise NoReading("vm_stat: no page size")
    pages = {"page_size": int(size.group(1))}
    for label, value in re.findall(r'^"?([^":\n]+)"?:\s+(\d+)\.', text, re.M):
        pages[label.strip()] = int(value)
    for need in ("Anonymous pages", "Pages wired down", "Pages occupied by compressor"):
        if need not in pages:
            raise NoReading(f"vm_stat: no {need!r}")
    return pages


@dataclass
class Sysctl:
    load1: float
    cores: int
    physical: int
    swap_used: int
    pressure: int


_UNIT = {"K": 1024, "M": 1024**2, "G": GIB}


def parse_sysctl(text: str) -> Sysctl:
    lines = [line for line in text.splitlines() if line.strip()]
    if len(lines) != 5:
        raise NoReading(f"sysctl: {len(lines)} lines, expected 5")
    try:
        load1 = float(lines[0].strip("{} ").split()[0])
        swap = re.search(r"used = ([\d.]+)([KMG])", lines[3])
        if not swap:
            raise ValueError(lines[3])
        cores = int(lines[1])
        if cores < 1:
            raise ValueError(f"hw.ncpu {cores}")
        return Sysctl(
            load1=load1,
            cores=cores,
            physical=int(lines[2]),
            swap_used=int(float(swap.group(1)) * _UNIT[swap.group(2)]),
            pressure=int(lines[4]),
        )
    except (ValueError, IndexError) as exc:
        raise NoReading(f"sysctl: {exc}") from exc


@dataclass
class Proc:
    pid: int
    ppid: int
    rss_kib: int
    elapsed_s: int
    stat: str
    comm: str


def parse_etime(text: str) -> int:
    """``[[dd-]hh:]mm:ss`` to seconds."""
    days, _, rest = text.rpartition("-")
    parts = [int(p) for p in rest.split(":")]
    while len(parts) < 3:
        parts.insert(0, 0)
    hours, minutes, seconds = parts
    return ((int(days or 0) * 24 + hours) * 60 + minutes) * 60 + seconds


def parse_ps(text: str) -> list[Proc]:
    table = []
    for line in text.splitlines():
        fields = line.split(None, 5)
        if len(fields) < 6:
            continue
        try:
            table.append(
                Proc(
                    pid=int(fields[0]),
                    ppid=int(fields[1]),
                    rss_kib=int(fields[2]),
                    elapsed_s=parse_etime(fields[3]),
                    stat=fields[4],
                    comm=os.path.basename(fields[5].strip()),
                )
            )
        except ValueError:
            continue
    if not table:
        raise NoReading("ps: empty process table")
    return table


def step_of(comm: str, args: str) -> str:
    """The cargo step an invocation runs, from its argv."""
    if comm.startswith("cargo-"):
        sub = comm[len("cargo-") :]
        if sub in STEPS:
            return sub
    for word in args.split()[1:]:
        if word.startswith(("-", "+")):
            continue
        return word if word in STEPS else "other"
    return "other"


# --- the build queue -----------------------------------------------------


def is_cargo(comm: str) -> bool:
    return (comm == "cargo" or comm.startswith("cargo-")) and comm not in NOT_ROOTS


@dataclass
class Build:
    pid: int
    step: str
    elapsed_s: int
    compilers: Counter = field(default_factory=Counter)


@dataclass
class Queue:
    building: list[Build]
    waiting: list[Build]

    @property
    def longest(self) -> Build | None:
        return max(self.waiting, key=lambda b: b.elapsed_s, default=None)


def cargo_roots(table: list[Proc]) -> list[Proc]:
    by_pid = {p.pid: p for p in table}
    return [
        p
        for p in table
        if is_cargo(p.comm)
        and not p.stat.startswith("T")
        and not (p.ppid in by_pid and is_cargo(by_pid[p.ppid].comm))
    ]


def build_queue(table: list[Proc], args: dict[int, str]) -> Queue:
    children: dict[int, list[Proc]] = {}
    for p in table:
        children.setdefault(p.ppid, []).append(p)
    building, waiting = [], []
    for root in cargo_roots(table):
        build = Build(
            root.pid, step_of(root.comm, args.get(root.pid, "")), root.elapsed_s
        )
        working = False
        stack = list(children.get(root.pid, []))
        while stack:
            child = stack.pop()
            if not is_cargo(child.comm):
                working = True
                if child.comm in COMPILERS:
                    build.compilers[child.comm] += 1
            stack.extend(children.get(child.pid, []))
        (building if working else waiting).append(build)
    return Queue(building, waiting)


def parse_args_ps(text: str) -> dict[int, str]:
    out = {}
    for line in text.splitlines():
        pid, _, rest = line.strip().partition(" ")
        if pid.isdigit():
            out[int(pid)] = rest.strip()
    return out


# --- sessions ------------------------------------------------------------


@dataclass
class Sessions:
    live: int
    building: int
    free: int
    records: int


def read_sessions(table: list[Proc], state_dir: Path | None) -> Sessions:
    live = {p.pid for p in table if p.comm == "claude"}
    classes: dict[int, str] = {}
    if state_dir and state_dir.is_dir():
        for path in sorted(state_dir.rglob("*.class")):
            try:
                cls, pid = path.read_text().split()
                if int(pid) in live:
                    classes[int(pid)] = cls
            except (OSError, ValueError):
                # Unreadable, undecodable or malformed: not a live record.
                continue
    free = sum(1 for cls in classes.values() if cls == "free")
    return Sessions(
        live=len(live), building=len(live) - free, free=free, records=len(classes)
    )


# --- the reading ---------------------------------------------------------


@dataclass
class Memory:
    physical: int
    app: int
    wired: int
    compressed: int

    @property
    def in_use(self) -> int:
        return self.app + self.wired + self.compressed


@dataclass
class Reading:
    memory: Memory
    sysctl: Sysctl
    queue: Queue
    sessions: Sessions
    docker_rss: int | None


def read_machine(runner: Runner = run, state_dir: Path | None = None) -> Reading:
    pages = parse_vm_stat(runner(["vm_stat"]))
    ctl = parse_sysctl(runner(SYSCTL))
    table = parse_ps(runner(PS_TABLE))
    roots = cargo_roots(table)
    args = {}
    if roots:
        pids = ",".join(str(p.pid) for p in roots)
        try:
            args = parse_args_ps(runner(["ps", "-ww", "-o", "pid=,args=", "-p", pids]))
        except NoReading:
            # A root that exited between the two reads makes ps exit 1, and
            # every step then reads `other` (bar a `cargo-<step>` comm)
            # rather than losing the whole reading.
            args = {}
    size = pages["page_size"]
    memory = Memory(
        physical=ctl.physical,
        app=pages["Anonymous pages"] * size,
        wired=pages["Pages wired down"] * size,
        compressed=pages["Pages occupied by compressor"] * size,
    )
    vm = [p.rss_kib * 1024 for p in table if p.comm == DOCKER_VM]
    return Reading(
        memory=memory,
        sysctl=ctl,
        queue=build_queue(table, args),
        sessions=read_sessions(table, state_dir),
        docker_rss=sum(vm) if vm else None,
    )


# --- knobs and verdict ---------------------------------------------------


@dataclass
class Knobs:
    build_wait_s: float
    total: int
    reserve: int
    cpu_load_max: float
    notes: list[str] = field(default_factory=list)


def _number(raw: str, cast: Callable[[str], float]) -> float:
    value = cast(raw)
    # NaN would slip past `< 0` and silently disable its check.
    if not math.isfinite(value) or value < 0:
        raise ValueError(raw)
    return value


def read_knobs(env, physical: int) -> Knobs:
    """The knobs from `env`; an unset or malformed required one raises
    `NoReading` naming the variable, so every caller refuses alike."""
    notes: list[str] = []

    def required(name: str, cast: Callable[[str], float]) -> float:
        raw = env.get(name, "").strip()
        if not raw:
            raise NoReading(f"{name} is unset; set it in the runtime config")
        try:
            return _number(raw, cast)
        except ValueError:
            raise NoReading(f"{name}={raw!r} is not a non-negative number") from None

    def knob(name: str, default: float, cast: Callable[[str], float]) -> float:
        raw = env.get(name, "").strip()
        if not raw:
            return default
        try:
            return _number(raw, cast)
        except ValueError:
            notes.append(
                f"{name}={raw!r} is not a non-negative number; using {default:g}"
            )
            return default

    return Knobs(
        build_wait_s=knob("DS_FLEET_BUILD_WAIT_S", 120, float),
        total=int(required("DS_FLEET_TOTAL", int)),
        reserve=int(knob("DS_FLEET_RESERVE_GB", physical / 8 / GIB, float) * GIB),
        cpu_load_max=required("DS_FLEET_CPU_LOAD_MAX", float),
        notes=notes,
    )


def machine_refusals(r: Reading, k: Knobs) -> list[str]:
    """The checks that refuse every class, as reason keys."""
    reasons = []
    if r.sysctl.pressure > 1:
        reasons.append("pressure")
    if r.sysctl.load1 > k.cpu_load_max:
        reasons.append("load")
    if r.memory.in_use > r.memory.physical - k.reserve:
        reasons.append("memory")
    return reasons


def room(r: Reading, k: Knobs) -> int:
    """Clients the total still admits, whatever their class."""
    return max(0, k.total - r.sessions.live)


def verdict(r: Reading, k: Knobs, cls: str) -> tuple[int, list[str]]:
    """``(slots, reasons)`` for one class."""
    reasons = machine_refusals(r, k)
    left = room(r, k)
    if left == 0:
        reasons.append("total")
    if cls == "building":
        longest = r.queue.longest
        if longest and longest.elapsed_s >= k.build_wait_s:
            reasons.append("wait")
    if reasons:
        return 0, reasons
    if cls == "building" and r.queue.waiting:
        return min(1, left), []
    return left, []


def status(slots: int, asked: int) -> int:
    if slots >= asked:
        return GREEN
    return AMBER if slots > 0 else RED


# --- rendering -----------------------------------------------------------


def duration(seconds: float) -> str:
    seconds = int(seconds)
    if seconds >= 3600:
        return f"{seconds // 3600}h{seconds % 3600 // 60:02d}m"
    return f"{seconds // 60}m{seconds % 60:02d}s"


def size(n: int) -> str:
    for unit, scale in (("GiB", GIB), ("MiB", 1024**2), ("KiB", 1024)):
        if n >= scale:
            return f"{n / scale:.1f} {unit}"
    return f"{n} B"


def gib(n: int) -> str:
    return f"{n / GIB:.1f}"


def headroom(r: Reading, k: Knobs) -> int:
    return max(0, r.memory.physical - k.reserve - r.memory.in_use)


def remedy(reason: str, r: Reading, k: Knobs) -> str:
    return {
        "wait": f"the longest wait falling under {duration(k.build_wait_s)}",
        "total": f"a session closing (total {k.total})",
        "memory": "memory headroom",
        "pressure": "kernel pressure back to normal",
        "load": f"load falling under {k.cpu_load_max:g}",
    }[reason]


def refusal(reason: str, r: Reading, k: Knobs) -> str:
    return {
        "total": f"clients ({r.sessions.live} of {k.total})",
        "memory": f"memory (headroom {gib(headroom(r, k))} GiB)",
        "pressure": f"kernel pressure {PRESSURE.get(r.sysctl.pressure, r.sysctl.pressure)}",
        "load": f"load ({r.sysctl.load1:.1f}, limit {k.cpu_load_max:g})",
    }[reason]


def render(r: Reading, k: Knobs, cls: str, asked: int, asked_given: bool) -> str:
    slots, reasons = verdict(r, k, cls)
    head = f"{slots} slot{'' if slots == 1 else 's'} for {cls}"
    if asked_given:
        head += f" ({asked} asked)"
    q, m, s = r.queue, r.memory, r.sysctl
    built = "; ".join(
        b.step + "".join(f", {n} {c}" for c, n in sorted(b.compilers.items()))
        for b in q.building
    )
    lines = [
        f"machine-load: {COLOUR[status(slots, asked)]} - {head}",
        f"  builds    {len(q.building)} building"
        + (f" ({built})" if built else "")
        + f", {len(q.waiting)} waiting on the pool lock",
    ]
    if q.longest:
        by_step = Counter(b.step for b in q.waiting).most_common()
        lines.append(
            "            waiting: "
            + ", ".join(f"{n} {step}" for step, n in by_step)
            + f"; longest {duration(q.longest.elapsed_s)} (pid {q.longest.pid});"
            f" limit {duration(k.build_wait_s)}"
        )
    seats = r.sessions
    lines += [
        f"  clients   {seats.live} of {k.total}"
        f" (builders {seats.building}, free {seats.free})"
        + (
            ""
            if seats.records
            else " (no class records: every live claude counts as building)"
        ),
        f"  memory    {gib(m.in_use)} of {gib(m.physical)} GiB in use"
        f" (app {gib(m.app)}, wired {gib(m.wired)}, compressed {gib(m.compressed)})",
        f"            reserve {gib(k.reserve)} -> headroom {gib(headroom(r, k))} GiB",
        f"  load      {s.load1:.1f} on {s.cores} cores (limit {k.cpu_load_max:g})",
        f"  swap      {size(s.swap_used)}; kernel pressure"
        f" {PRESSURE.get(s.pressure, f'level {s.pressure}')}",
        "  docker    "
        + (f"VM {size(r.docker_rss)} resident" if r.docker_rss else "VM not running"),
    ]
    building_slots, building_reasons = verdict(r, k, "building")
    if building_slots == 0:
        lines.append(
            "  would free a slot: "
            + "; ".join(remedy(x, r, k) for x in building_reasons)
        )
    free_slots, free_reasons = verdict(r, k, "free")
    lines.append(
        "  free: "
        + (
            "refused on " + "; ".join(refusal(x, r, k) for x in free_reasons)
            if free_reasons
            else f"{free_slots} slot{'' if free_slots == 1 else 's'}"
        )
    )
    lines += [f"  note: {note}" for note in k.notes]
    return "\n".join(lines)


def as_json(r: Reading, k: Knobs, cls: str, asked: int) -> dict:
    slots, _ = verdict(r, k, cls)
    longest = r.queue.longest
    return {
        "status": COLOUR[status(slots, asked)].lower(),
        "exit": status(slots, asked),
        "asked": {"class": cls, "sessions": asked},
        "verdict": {
            c: dict(zip(("slots", "reasons"), verdict(r, k, c)))
            for c in ("building", "free")
        },
        "builds": {
            "building": [
                {"pid": b.pid, "step": b.step, "compilers": dict(b.compilers)}
                for b in r.queue.building
            ],
            "waiting": [
                {"pid": b.pid, "step": b.step, "wait_s": b.elapsed_s}
                for b in r.queue.waiting
            ],
            "longest_wait_s": longest.elapsed_s if longest else 0,
        },
        "sessions": asdict(r.sessions),
        "memory": {
            **asdict(r.memory),
            "in_use": r.memory.in_use,
            "reserve": k.reserve,
            "headroom": headroom(r, k),
        },
        "load": {
            "load1": r.sysctl.load1,
            "cores": r.sysctl.cores,
            "max": k.cpu_load_max,
        },
        "swap_used": r.sysctl.swap_used,
        "pressure": PRESSURE.get(r.sysctl.pressure, f"level {r.sysctl.pressure}"),
        "docker_rss": r.docker_rss,
        "knobs": asdict(k),
    }


def sample_line(r: Reading, now: datetime) -> str:
    waiting = Counter(b.step for b in r.queue.waiting)
    longest = r.queue.longest
    return " ".join(
        [
            now.strftime("%Y-%m-%dT%H:%M:%SZ"),
            f"builders={r.sessions.building}",
            f"builds={len(r.queue.building)}",
            "waiting="
            + (",".join(f"{s}:{n}" for s, n in sorted(waiting.items())) or "-"),
            f"longest_s={longest.elapsed_s if longest else 0}",
            f"in_use_gib={r.memory.in_use / GIB:.1f}",
        ]
    )


def record_line(
    r: Reading | None, k: Knobs | None, cls: str, now: datetime, env, error: str = ""
) -> str:
    """One launch record: the reading a session started or resumed under, as
    one compact JSON line for its ``sessions/<id>.load``. A no-reading launch
    still leaves a line, so a gap in the record is never silent."""
    lanes = env.get("DS_CARGO_LANES") or str(DEFAULT_LANES)
    rec: dict = {
        "at": now.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "class": cls,
        "lanes": int(lanes) if lanes.isdecimal() else None,
    }
    if r is None or k is None:
        return json.dumps(
            {**rec, "status": "no-reading", "error": error}, separators=(",", ":")
        )
    slots, reasons = verdict(r, k, cls)
    longest = r.queue.longest
    rec.update(
        {
            "status": COLOUR[status(slots, 1)].lower(),
            "queue_building": len(r.queue.building),
            "queue_waiting": len(r.queue.waiting),
            "longest_wait_s": longest.elapsed_s if longest else 0,
            "in_use": r.memory.in_use,
            "compressed": r.memory.compressed,
            "load1": r.sysctl.load1,
            "swap_used": r.sysctl.swap_used,
            "live": r.sessions.live,
            "building": r.sessions.building,
            "free": r.sessions.free,
            "verdict": {"slots": slots, "reasons": reasons},
        }
    )
    return json.dumps(rec, separators=(",", ":"))


def append_record(path: Path, line: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "a") as out:
        out.write(line + "\n")


def default_state_dir() -> Path:
    """The base checkout's substrate directory, from this file's checkout."""
    checkout = str(Path(__file__).resolve().parents[2])
    head, sep, _ = (checkout + "/").partition(WORKTREE_SEGMENT)
    return Path(head if sep else checkout) / STATE_DIR


class _Parser(argparse.ArgumentParser):
    """argparse exits 2 on a usage error, which a caller would read as RED."""

    def error(self, message: str):
        self.print_usage(sys.stderr)
        self.exit(NO_READING, f"machine-load: NO READING - {message}\n")


def no_reading(reason: str, as_json: bool) -> int:
    if as_json:
        print(json.dumps({"status": "no-reading", "exit": NO_READING, "error": reason}))
    else:
        print(
            f"machine-load: NO READING - {reason}; fall back to the total",
            file=sys.stderr,
        )
    return NO_READING


def _record_no_reading(opts, now: datetime, env, reason: str) -> None:
    if opts.record:
        try:
            append_record(
                opts.record, record_line(None, None, opts.cls, now, env, reason)
            )
        except OSError:
            pass


def main(argv: list[str] | None = None, runner: Runner = run, env=None) -> int:
    parser = _Parser(description=__doc__.splitlines()[0])
    parser.add_argument("--json", action="store_true", help="print the fields as JSON")
    parser.add_argument("--sessions", type=int, default=None, metavar="N")
    parser.add_argument(
        "--class", dest="cls", choices=("building", "free"), default="building"
    )
    parser.add_argument(
        "--sample", action="store_true", help="append one line to the fleet log"
    )
    parser.add_argument("--state-dir", type=Path, default=None)
    parser.add_argument(
        "--record",
        type=Path,
        default=None,
        metavar="PATH",
        help="append the launch record line to PATH (a session's .load)",
    )
    opts = parser.parse_args(argv)
    asked_given = opts.sessions is not None
    asked = opts.sessions if asked_given else 1
    if asked < 1:
        parser.error("--sessions must be at least 1")
    state_dir = opts.state_dir or default_state_dir()
    env = os.environ if env is None else env
    now = datetime.now(timezone.utc)
    try:
        if platform.system() != "Darwin" and runner is run:
            raise NoReading(f"not Darwin ({platform.system()})")
        reading = read_machine(runner, state_dir)
        knobs = read_knobs(env, reading.memory.physical)
        slots, _ = verdict(reading, knobs, opts.cls)
        out = (
            json.dumps(as_json(reading, knobs, opts.cls, asked), indent=2)
            if opts.json
            else render(reading, knobs, opts.cls, asked, asked_given)
        )
    except NoReading as exc:
        _record_no_reading(opts, now, env, str(exc))
        return no_reading(str(exc), opts.json)
    except Exception as exc:  # noqa: BLE001 — a crash must not exit 1 (AMBER)
        _record_no_reading(opts, now, env, f"{type(exc).__name__}: {exc}")
        return no_reading(f"{type(exc).__name__}: {exc}", opts.json)
    # After the reading, not inside it: a log that cannot be written is a
    # warning, never a no-reading verdict on a reading that succeeded.
    writes = []
    if opts.sample:
        writes.append((state_dir / SAMPLE_LOG, sample_line(reading, now)))
    if opts.record:
        writes.append((opts.record, record_line(reading, knobs, opts.cls, now, env)))
    for path, line in writes:
        try:
            append_record(path, line)
        except OSError as exc:
            print(f"machine-load: could not append to {path}: {exc}", file=sys.stderr)
    print(out)
    return status(slots, asked)


if __name__ == "__main__":
    raise SystemExit(main())
