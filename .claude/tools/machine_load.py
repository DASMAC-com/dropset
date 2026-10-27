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

The reading is three commands and one targeted read, no sampling loop:
``vm_stat``; ``sysctl -n vm.loadavg hw.ncpu hw.memsize vm.swapusage
kern.memorystatus_vm_pressure_level``; ``ps -axo pid=,ppid=,rss=,etime=,comm=``;
and one ``ps -o args=`` over the cargo pids only. Never a pattern over every
process — ``pgrep -f claude`` matches every shell whose command line names a
``.claude`` path.

**Build queue.** A cargo invocation is the topmost process of a cargo chain
(``cargo``, ``cargo-*``; ``cargo-clippy`` runs a child ``cargo``, which is the
same invocation). It is *building* when anything that is not cargo runs under
it — ``rustc`` / ``rustdoc``, a test binary, the linker — and *waiting* when it
holds no such descendant: a cargo blocked on the pool lock has spent under a
second of CPU, so its age is its wait. Each is named by its step from its
args (``clippy``, ``doc``, ``test``, ``nextest``, ``build``, ``check``,
``run``).

**Memory in use** is anonymous + wired + compressor-occupied pages times the
page size — Activity Monitor's "Memory Used", so the operator can check the
tool against what they already look at. File cache and free pages are
headroom.

**Sessions by class** come from the launch records in the session-substrate
directory: a ``<key>.class`` file beside a substrate marker holds ``building``
or ``free``. Until any record exists every live ``claude`` counts as building;
the free count is capped at the live count, so a stale record can never
manufacture a session.

**Knobs** come from the environment (the operator's untracked runtime config);
the repo carries no machine literal: ``DS_FLEET_BUILD_WAIT_S`` (120),
``DS_FLEET_CAP`` (20), ``DS_FLEET_RESERVE_GB`` (physical / 8),
``DS_FLEET_LOAD_PER_CORE`` (1.5).

Exit 0 green (every asked session fits), 1 amber (fewer fit), 2 red (none),
3 no reading (not Darwin, a command or parse failure). A caller never halts
on 3: it falls back to the cap and says so.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_machine_load.py``.
"""

from __future__ import annotations

import argparse
import json
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

STEPS = ("clippy", "doc", "test", "nextest", "build", "check", "run")
COMPILERS = ("rustc", "rustdoc", "clippy-driver")
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
PS_TABLE = ["ps", "-axc", "-o", "pid=,ppid=,rss=,etime=,comm="]

GREEN, AMBER, RED, NO_READING = 0, 1, 2, 3
COLOUR = {GREEN: "GREEN", AMBER: "AMBER", RED: "RED"}


class NoReading(Exception):
    """The machine could not be read; the caller falls back to the cap."""


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
        return Sysctl(
            load1=load1,
            cores=int(lines[1]),
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
        fields = line.split(None, 4)
        if len(fields) < 5:
            continue
        try:
            table.append(
                Proc(
                    pid=int(fields[0]),
                    ppid=int(fields[1]),
                    rss_kib=int(fields[2]),
                    elapsed_s=parse_etime(fields[3]),
                    comm=os.path.basename(fields[4].strip()),
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
    return comm == "cargo" or comm.startswith("cargo-")


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
        if is_cargo(p.comm) and not (p.ppid in by_pid and is_cargo(by_pid[p.ppid].comm))
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
    live = sum(1 for p in table if p.comm == "claude")
    classes = []
    if state_dir and state_dir.is_dir():
        for path in sorted(state_dir.rglob("*.class")):
            try:
                classes.append(path.read_text().strip())
            except OSError:
                continue
    free = min(classes.count("free"), live)
    return Sessions(live=live, building=live - free, free=free, records=len(classes))


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
            # A root that exited between the two reads makes ps exit 1; its
            # step reads `other` rather than losing the whole reading.
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
    cap: int
    reserve: int
    load_per_core: float
    notes: list[str] = field(default_factory=list)


def read_knobs(env: dict[str, str], physical: int) -> Knobs:
    notes: list[str] = []

    def knob(name: str, default: float, cast: Callable[[str], float]) -> float:
        raw = env.get(name, "").strip()
        if not raw:
            return default
        try:
            value = cast(raw)
            if value < 0:
                raise ValueError
            return value
        except ValueError:
            notes.append(
                f"{name}={raw!r} is not a non-negative number; using {default:g}"
            )
            return default

    return Knobs(
        build_wait_s=knob("DS_FLEET_BUILD_WAIT_S", 120, float),
        cap=int(knob("DS_FLEET_CAP", 20, int)),
        reserve=int(knob("DS_FLEET_RESERVE_GB", physical / 8 / GIB, float) * GIB),
        load_per_core=knob("DS_FLEET_LOAD_PER_CORE", 1.5, float),
        notes=notes,
    )


def machine_refusals(r: Reading, k: Knobs) -> list[str]:
    """The checks that refuse every class, as reason keys."""
    reasons = []
    if r.sysctl.swap_used > 0:
        reasons.append("swap")
    if r.sysctl.pressure > 1:
        reasons.append("pressure")
    if r.sysctl.load1 / r.sysctl.cores > k.load_per_core:
        reasons.append("load")
    if r.memory.in_use > r.memory.physical - k.reserve:
        reasons.append("memory")
    return reasons


def verdict(r: Reading, k: Knobs, cls: str) -> tuple[int | None, list[str]]:
    """``(slots, reasons)``; ``None`` slots is unbounded."""
    reasons = machine_refusals(r, k)
    if cls == "free":
        return (0, reasons) if reasons else (None, [])
    longest = r.queue.longest
    if longest and longest.elapsed_s >= k.build_wait_s:
        reasons.append("wait")
    room = max(0, k.cap - r.sessions.building)
    if room == 0:
        reasons.append("cap")
    if reasons:
        return 0, reasons
    return (min(1, room) if r.queue.waiting else room), []


def status(slots: int | None, asked: int) -> int:
    if slots is None or slots >= asked:
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
        "cap": f"a building session closing (cap {k.cap})",
        "memory": "memory headroom",
        "swap": "swap draining to 0",
        "pressure": "kernel pressure back to normal",
        "load": f"load falling under {k.load_per_core:.2f} per core",
    }[reason]


def refusal(reason: str, r: Reading, k: Knobs) -> str:
    return {
        "memory": f"memory (headroom {gib(headroom(r, k))} GiB)",
        "swap": f"swap ({size(r.sysctl.swap_used)} used)",
        "pressure": f"kernel pressure {PRESSURE.get(r.sysctl.pressure, r.sysctl.pressure)}",
        "load": f"load ({r.sysctl.load1 / r.sysctl.cores:.2f} per core)",
    }[reason]


def slots_text(slots: int | None) -> str:
    return "unbounded" if slots is None else str(slots)


def render(r: Reading, k: Knobs, cls: str, asked: int, asked_given: bool) -> str:
    slots, reasons = verdict(r, k, cls)
    head = f"{slots_text(slots)} slots for {cls}"
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
        f"  sessions  {seats.building} building of cap {k.cap}; {seats.free} free"
        + (
            ""
            if seats.records
            else " (no class records: every live claude counts as building)"
        ),
        f"  memory    {gib(m.in_use)} of {gib(m.physical)} GiB in use"
        f" (app {gib(m.app)}, wired {gib(m.wired)}, compressed {gib(m.compressed)})",
        f"            reserve {gib(k.reserve)} -> headroom {gib(headroom(r, k))} GiB",
        f"  load      {s.load1:.1f} on {s.cores} cores = {s.load1 / s.cores:.2f} per core"
        f" (limit {k.load_per_core:.2f})",
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
            "unbounded"
            if free_slots is None
            else "refused on " + "; ".join(refusal(x, r, k) for x in free_reasons)
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
            "per_core": r.sysctl.load1 / r.sysctl.cores,
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


def default_state_dir() -> Path:
    """The base checkout's substrate directory, from this file's checkout."""
    checkout = str(Path(__file__).resolve().parents[2])
    head, sep, _ = (checkout + "/").partition(WORKTREE_SEGMENT)
    return Path(head if sep else checkout) / STATE_DIR


def main(argv: list[str] | None = None, runner: Runner = run, env=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--json", action="store_true", help="print the fields as JSON")
    parser.add_argument("--sessions", type=int, default=None, metavar="N")
    parser.add_argument(
        "--class", dest="cls", choices=("building", "free"), default="building"
    )
    parser.add_argument(
        "--sample", action="store_true", help="append one line to the fleet log"
    )
    parser.add_argument("--state-dir", type=Path, default=None)
    opts = parser.parse_args(argv)
    asked_given = opts.sessions is not None
    asked = opts.sessions if asked_given else 1
    if asked < 1:
        parser.error("--sessions must be at least 1")
    state_dir = opts.state_dir or default_state_dir()
    try:
        if platform.system() != "Darwin" and runner is run:
            raise NoReading(f"not Darwin ({platform.system()})")
        reading = read_machine(runner, state_dir)
    except NoReading as exc:
        if opts.json:
            print(
                json.dumps(
                    {"status": "no-reading", "exit": NO_READING, "error": str(exc)}
                )
            )
        else:
            print(
                f"machine-load: NO READING - {exc}; fall back to the cap",
                file=sys.stderr,
            )
        return NO_READING
    knobs = read_knobs(os.environ if env is None else env, reading.memory.physical)
    if opts.sample:
        state_dir.mkdir(parents=True, exist_ok=True)
        with open(state_dir / SAMPLE_LOG, "a") as log:
            log.write(sample_line(reading, datetime.now(timezone.utc)) + "\n")
    if opts.json:
        print(json.dumps(as_json(reading, knobs, opts.cls, asked), indent=2))
    else:
        print(render(reading, knobs, opts.cls, asked, asked_given))
    slots, _ = verdict(reading, knobs, opts.cls)
    return status(slots, asked)


if __name__ == "__main__":
    raise SystemExit(main())
