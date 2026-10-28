#!/usr/bin/env python3
"""Open a new iTerm tab per session verb and type the verb into it.

A board session's dispatch arm (the scheduler's, or the curator's for an
architect spin-off): it names a task that is ready to start,
offers to start it, and on the operator's yes runs this. The ask-then-yes IS
the authorization, the same class as `fleet go` — the one gate this tool adds is
the machine's (below), never a second ask.

    python3 .claude/tools/session_dispatch.py task 1234
    python3 .claude/tools/session_dispatch.py task anthropic 1234
    python3 .claude/tools/session_dispatch.py task 1234 + chore 1240 + plan
    python3 .claude/tools/session_dispatch.py --dry-run plan

**A dispatch opens a TAB in the dispatching session's own window**, never a new
window: the operator drives the fleet from one window, so a dispatched session
has to land beside the session that started it rather than as another
window to hunt for. Operator ruling, 2026-09-10, on seeing two resumes arrive as
separate windows. This reverses an earlier "one iTerm window per session is
load-bearing" rationale that used to be stated here, in the `plan` skill's step
5, and in the local-integrations convention doc.

A tab still never falls back to typing into the CURRENT session — the failure
path prints the verb instead. Typing a launch verb into a session that is
already running something remains the one outcome worse than not dispatching at
all, and that hazard was never what the window-per-session rule bought.

Several verbs in one invocation are separated by a bare `+`, which keeps the
one-verb form untouched and costs one driver round trip for the whole batch
rather than one per tab. `+` is safe as a separator precisely because the
grammar below refuses it as a verb or an argument.

WHY PURE PYTHON AND NOT APPLESCRIPT. The machine is Apple-only and single-user,
and iTerm's Python API is its supported automation surface; the AppleScript path
this replaces is one more language in the toolbox for no capability gained. Best
effort is the ratified bar — getting it working beats getting it bulletproof —
which is why every failure path below prints the verb it *would* have typed, so
the operator can paste it by hand and lose nothing but the convenience.

The iTerm driving itself lives in `iterm_api`, shared with `fleet_resume.py`;
see there for the interpreter dance that keeps this repo's own Python
stdlib-only while still reaching a library that is not.

**THE MACHINE GATE.** Before any tab opens, every verb is classed — `free`
(no cargo, no Docker, no PR) or `building` — and the machine-load probe
(`machine_load.py`) is read once for the batch. What fits is seated; the rest
is printed as `held` with the number that refused it, and a scheduler session
re-dispatches it when a slot opens. That reading replaces the session count
that used to live only in doctrine; the count survives as `DS_FLEET_CAP`, which
the probe applies and nothing here overrides. `--over-budget` seats past every
other check, as the operator's word in their own tab. A probe with no reading
(exit 3) never halts the fleet: the batch is seated under the cap alone, and
the dispatch says why. `fleet_resume.py` runs the same gate over its resume
list, which is why the gate is public here rather than inlined in `run`.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

import iterm_api
import linear_issue
import machine_load

#: The verbs this can dispatch, each mapped to a validator for its arguments.
#:
#: THIS IS INPUT VALIDATION, NOT POLICY. The ratified design says the dispatcher
#: takes the verb and its arguments verbatim and adds no policy of its own —
#: which it does: a substrate word passes straight through, so the choice stays
#: at the call site where it belongs. What this table refuses is not a *choice*
#: but a *shape*: the tool types its argument into an interactive shell, so
#: anything that is not one of these forms would be arbitrary command execution
#: wearing a verb's name. A caller wanting a shell command has a shell.
#: `\Z` and `re.ASCII`, both deliberately, and neither is theoretical.
#: Python's `$` matches just before a trailing newline, so `^\d+$` accepts
#: `"1234\n"` — a different shape than the shell's grammar takes, reaching a
#: line that gets typed into a live shell. And bare `\d` is Unicode-aware, so
#: `eng-١٢٣` would validate as a tag. `shlex.quote` downstream keeps either
#: from being exploitable; the point of this table is to refuse the shape.
_TAG = re.compile(r"\A(?:eng-)?\d+\Z", re.ASCII)
_NAME = re.compile(r"\A[a-z0-9][a-z0-9-]*\Z", re.ASCII)

#: The substrate override words, the same on every verb that takes one.
_SUBSTRATES = ("anthropic", "bedrock")

#: The daily seat verbs, each mapped to the iTerm role its tab will carry —
#: which places the tab (`iterm_api.insert_index`): the scheduler's rightmost,
#: the others just left of it. Every other verb's tab is a worker's.
SEAT_VERBS = {
    "plan": iterm_api.PLAN_ROLE,
    "curate": iterm_api.CURATE_ROLE,
    "launch": iterm_api.LAUNCH_ROLE,
}


def validate(argv: list[str]) -> list[str]:
    """Return the verb words to type, or raise ``ValueError`` naming the problem.

    Mirrors the grammar in `.claude/shell/init.zsh`. The two must agree, and the
    failure of disagreement is one-directional and cheap: a form this rejects
    that the shell accepts costs the operator a hand-typed launch, while a form
    this accepts that the shell rejects costs a confused error in a fresh
    window. Neither is silent, which is what makes duplicating the grammar
    acceptable rather than a second source of truth to keep in sync.
    """
    if not argv:
        raise ValueError("no verb given")

    verb, args = argv[0], argv[1:]

    if verb == "task":
        if args and args[0] == "local":
            # The shell still takes it, with a deprecation line; a dispatch is
            # generated text, so it should already speak the new word.
            raise ValueError("`task local` is retired — dispatch `task anthropic <n>`")
        if args and args[0] in (*_SUBSTRATES, "resume"):
            sub, rest = args[0], args[1:]
            # `task resume` alone is the picker, which is a legitimate thing to
            # dispatch; `task anthropic` alone is not, since there is no
            # worktree to open. The shell says the same.
            if sub == "resume" and not rest:
                return ["task", "resume"]
            if len(rest) != 1 or not _TAG.match(rest[0]):
                raise ValueError(f"`task {sub}` takes one eng-### tag or number")
            return ["task", sub, rest[0]]
        if len(args) != 1 or not _TAG.match(args[0]):
            raise ValueError("`task` takes one eng-### tag or number")
        return ["task", args[0]]

    if verb == "explore":
        # A name is REQUIRED (it names a worktree) and the `resume` twin is
        # retired, since `explore <n|name>` now resumes an existing session
        # itself. `_NAME` admits a bare number, which is the issue-keyed form an
        # audit dispatch uses — `explore 1196` lands in the `1196` worktree.
        # Either substrate word, in `task`'s position, so a `[Explore/Bedrock]`
        # title types as `explore bedrock <n>` like every other token; the
        # shell refuses either word as a name, so `explore bedrock` alone is
        # no `exp-bedrock`.
        word = [args[0]] if args and args[0] in _SUBSTRATES else []
        args = args[len(word) :]
        if args and args[0] == "resume":
            raise ValueError(
                "`explore resume` is retired — dispatch `explore <n|name>`, "
                "which resumes an existing session itself"
            )
        if len(args) != 1 or not _NAME.match(args[0]) or args[0] in _SUBSTRATES:
            raise ValueError(
                "`explore` takes [anthropic|bedrock] and one lowercase name or "
                "issue number"
            )
        # `_NAME` admits `0` and `eng-0`; the shell refuses both, since 0 is not a
        # Linear issue number. Mirror that here rather than letting the dispatch
        # succeed: it would open a tab, type the verb, and leave a session that
        # LOOKS started and is idle — the exact silent failure the issue-keyed
        # bootstrap prompt exists to remove. It would also break the documented
        # all-or-nothing property, since one bad verb must dispatch none.
        if re.fullmatch(r"(?:eng-)?0+", args[0], re.ASCII):
            raise ValueError("`explore` needs a real issue number; 0 is not one")
        return ["explore", *word, args[0]]

    if verb == "chore":
        # `task`'s substrate words, but no `resume`: `chore <n>` resumes an
        # existing session itself, as `explore` does.
        if args and args[0] in _SUBSTRATES:
            args = args[1:]
        if len(args) != 1 or not _TAG.match(args[0]):
            raise ValueError("`chore` takes one eng-### tag or number")
        if re.fullmatch(r"(?:eng-)?0+", args[0], re.ASCII):
            raise ValueError("`chore` needs a real issue number; 0 is not one")
        return argv[:]

    if verb == "architect":
        # The governing issue's number is the only argument — there is no topic
        # form — so a design thread is named, tabbed and worktree'd like a task.
        # The substrate word comes BEFORE the number, as on every issue verb.
        if len(args) == 2 and _TAG.match(args[0]) and args[1] in _SUBSTRATES:
            raise ValueError(
                f"`architect <n> {args[1]}` is the retired order — dispatch "
                f"`architect {args[1]} {args[0]}`"
            )
        word = [args[0]] if args and args[0] in _SUBSTRATES else []
        rest = args[len(word) :]
        if len(rest) != 1 or not _TAG.match(rest[0]):
            raise ValueError(
                "`architect` takes [anthropic|bedrock] and the governing issue's number"
            )
        if re.fullmatch(r"(?:eng-)?0+", rest[0], re.ASCII):
            raise ValueError("`architect` needs a real issue number; 0 is not one")
        return ["architect", *word, rest[0]]

    if verb == "fleet":
        if not args:
            return ["fleet"]
        if args == ["go"]:
            return ["fleet", "go"]
        raise ValueError("`fleet` takes nothing or `go`")

    if verb in SEAT_VERBS:
        # The daily seat verbs: the day names the session, so no number.
        if len(args) > 1 or (args and args[0] not in _SUBSTRATES):
            raise ValueError(f"`{verb}` takes nothing, or anthropic or bedrock")
        return [verb, *args]

    raise ValueError(f"unknown verb {verb!r}")


#: Separates one verb from the next in a batch dispatch.
#:
#: A separator rather than inference, deliberately. The verb names are a closed
#: set, so consecutive verbs look as if they could be split apart without one —
#: but `explore` takes a
#: lowercase name and `_NAME` matches `plan`, so `explore plan` is
#: genuinely ambiguous between one verb and two. On a boundary that types into a
#: live shell, an explicit separator beats a heuristic that is right most of the
#: time. `+` is not a legal verb or argument under the grammar above, so it can
#: never collide with one; `--` was rejected because argparse strips a lone `--`
#: even out of a REMAINDER.
VERB_SEPARATOR = "+"


def split_verbs(words: list[str]) -> list[list[str]]:
    """Split a flat word list into one word list per verb, on `+`.

    Raises ``ValueError`` on an empty group — a doubled, leading, or trailing
    separator. That is a typo rather than a request, and the alternative (quietly
    dropping the empty group) would dispatch a batch of a different size than the
    caller wrote, which is the sort of near-miss this tool must not paper over.
    """
    groups: list[list[str]] = [[]]
    for word in words:
        if word == VERB_SEPARATOR:
            groups.append([])
        else:
            groups[-1].append(word)

    if len(groups) > 1 and any(not group for group in groups):
        raise ValueError(
            f"empty verb around a `{VERB_SEPARATOR}` separator — "
            f"write `verb {VERB_SEPARATOR} verb`, with a verb on each side"
        )
    return groups


def command_line(words: list[str]) -> str:
    """The exact text to type. Quoted defensively even though `validate` has
    already constrained every word to a shell-safe shape — the quoting is a
    property of this function, not of today's caller."""
    return " ".join(shlex.quote(word) for word in words)


FREE, BUILDING = "free", "building"

#: Each verb's class. `architect`, `explore` and the seat verbs run no cargo
#: and no Docker and open no PR, so only the machine checks can refuse them; `fleet`
#: is free because each session it resumes meets this same gate inside
#: `fleet_resume.py`. `chore` is absent on purpose — it is inspected
#: (`chore_class`) — and a verb added later is building until this says
#: otherwise, the direction that errs toward refusing.
VERB_CLASS = {
    "architect": FREE,
    "curate": FREE,
    "explore": FREE,
    "fleet": FREE,
    "launch": FREE,
    "plan": FREE,
    "task": BUILDING,
}

#: Where a chore body names a build or deploy command. Searched only inside
#: code spans and fenced blocks, where a body writes a command: in prose "make
#: sure" and "anchor on" are English, and matching them would class every chore
#: as building. The guards either side keep `anchor-v2` and `make_x.py` out
#: (`docker-compose` is named so the guard does not drop it). `localnet` has no
#: prose sense, so it counts anywhere.
_CODE = re.compile(r"```.*?```|~~~.*?~~~|`[^`\n]+`", re.DOTALL)
_BUILD_COMMAND = re.compile(
    r"(?<![\w./-])(make|cargo|anchor|docker-compose|docker)"
    r"(?:[ \t]+([\w:./-]+))?(?![\w-])",
    re.ASCII,
)
_LOCALNET = re.compile(r"\blocalnet\b", re.IGNORECASE)


def build_command(body: str) -> str | None:
    """The first build or deploy command a chore body names, or ``None``."""
    for span in _CODE.findall(body):
        if match := _BUILD_COMMAND.search(span):
            return " ".join(word for word in match.groups() if word)
    match = _LOCALNET.search(body)
    return match.group(0).lower() if match else None


def issue_number(tag: str) -> str:
    """``eng-0123`` → ``123``, the key the launcher's records use."""
    return str(int(tag.rsplit("-", 1)[-1]))


def fetch_body(number: str) -> str | None:
    """An issue's body through the issue tool's fetch, or ``None`` if unreadable.

    Planning's shell holds `LINEAR_API_KEY`; a shell without it, Linear being
    down, or any reply the tool rejects all read as unreadable, which the
    caller classes as building.
    """
    api_key = os.environ.get("LINEAR_API_KEY", "").strip()
    if not api_key:
        return None
    try:
        issue = linear_issue.fetch_issue_body(api_key, f"ENG-{number}")
    except Exception:  # noqa: BLE001 — every failure is "unreadable", by design
        return None
    return issue.get("description") or ""


@dataclass
class Seat:
    """One verb as the gate sees it."""

    command: str
    cls: str
    #: Shown after the command — `(free)`, `(builds: make demo)` — or empty
    #: for a verb whose class is its name (`task`).
    label: str = ""
    #: The issue number of a chore, whose class is written for the launcher.
    record: str | None = None

    @property
    def shown(self) -> str:
        return f"{self.command} {self.label}" if self.label else self.command


def classify(words: list[str], override: str | None = None, fetch=None) -> Seat:
    """Class one validated verb. ``override`` is `--class`, which binds chores."""
    fetch = fetch or fetch_body
    command = command_line(words)
    if words[0] != "chore":
        cls = VERB_CLASS.get(words[0], BUILDING)
        return Seat(command, cls, "(free)" if cls == FREE else "")
    number = issue_number(words[-1])
    if override:
        reason = "--class"
    else:
        body = fetch(number)
        reason = "body unreadable" if body is None else build_command(body)
        override = BUILDING if reason else FREE
    label = (
        f"(builds: {reason})"
        if override == BUILDING
        else ("(free, --class)" if reason == "--class" else "(free)")
    )
    return Seat(command, override, label, record=number)


def state_dir() -> Path:
    """The base checkout's session-substrate directory, the probe's own."""
    return machine_load.default_state_dir()


def write_records(seats: list[Seat], directory: Path) -> None:
    """Record each seated chore's class beside its substrate marker.

    The class alone, no pid: the launcher stamps `<class> <pid>` when its
    `claude` starts, reading the class from here, and a resume reads it back
    instead of re-inspecting. A record already holding this class is left
    alone, so re-dispatching a live chore keeps the pid its launcher stamped
    rather than dropping it from the probe's count. Best effort — a lost
    record costs a chore counted as building, never a dispatch.
    """
    for seat in seats:
        if seat.record is None:
            continue
        path = directory / f"{seat.record}.class"
        try:
            if path.read_text().split()[:1] == [seat.cls]:
                continue
        except OSError:
            pass
        try:
            directory.mkdir(parents=True, exist_ok=True)
            path.write_text(f"{seat.cls}\n")
        except OSError:
            continue


def recorded_class(directory: Path, number: str) -> str:
    """The class a session's record holds; building when there is none."""
    for key in (number, f"eng-{number}"):
        try:
            cls = (directory / f"{key}.class").read_text().split()[0]
        except (OSError, IndexError):
            continue
        if cls in (FREE, BUILDING):
            return cls
    return BUILDING


def probe(seats: list[Seat], directory: Path) -> dict:
    """One machine-load reading as the probe's JSON.

    Anything that is not a reading — exit 3, a crash, a timeout, output that is
    not the JSON shape — comes back as the probe's own no-reading shape, so the
    caller has one test (`"verdict" in reading`) rather than three.
    """
    building = sum(seat.cls == BUILDING for seat in seats)
    try:
        proc = subprocess.run(
            [
                sys.executable,
                str(Path(__file__).with_name("machine_load.py")),
                "--json",
                "--sessions",
                str(max(1, building)),
                "--state-dir",
                str(directory),
            ],
            capture_output=True,
            text=True,
            timeout=60,
        )
        reading = json.loads(proc.stdout)
        if isinstance(reading, dict) and ("verdict" in reading or "error" in reading):
            return reading
        error = "the probe printed no reading"
    except (OSError, subprocess.SubprocessError, ValueError) as exc:
        error = f"{type(exc).__name__}: {exc}"
    return {"status": "no-reading", "exit": machine_load.NO_READING, "error": error}


def _reason(key: str, reading: dict, cap: int) -> str:
    """The binding number behind one refusal key."""
    knobs = reading.get("knobs") or {}
    sessions = reading.get("sessions") or {}
    duration = machine_load.duration
    gib = machine_load.GIB
    if key == "wait":
        return (
            f"longest build wait {duration(reading['builds']['longest_wait_s'])} "
            f"on the pool lock (limit {duration(knobs['build_wait_s'])})"
        )
    if key == "cap":
        return (
            f"{sessions['building']} building of the cap {cap}"
            if sessions
            else f"the cap {cap} (no reading counted the live sessions)"
        )
    if key == "slots":
        waiting = (reading.get("builds") or {}).get("waiting")
        return (
            f"the reading admits {reading['verdict']['building']['slots']} more "
            "building session(s)"
            + (" (builds are waiting on the pool lock)" if waiting else "")
        )
    if key == "memory":
        memory = reading["memory"]
        return (
            f"memory headroom {memory['headroom'] / gib:.1f} GiB "
            f"(reserve {memory['reserve'] / gib:.1f})"
        )
    if key == "swap":
        return f"swap {machine_load.size(reading['swap_used'])} in use"
    if key == "pressure":
        return f"kernel pressure {reading['pressure']}"
    if key == "load":
        return (
            f"load {reading['load']['per_core']:.2f} per core "
            f"(limit {knobs['load_per_core']:.2f})"
        )
    return key


def _remedy(key: str, reading: dict) -> str:
    knobs = reading.get("knobs") or {}
    return {
        "wait": "the longest wait falling under "
        + machine_load.duration(knobs.get("build_wait_s", 0)),
        "memory": "memory headroom over the reserve",
        "swap": "swap draining to 0",
        "pressure": "kernel pressure back to normal",
        "load": f"load falling under {knobs.get('load_per_core', 0):.2f} per core",
    }.get(key, "")


@dataclass
class Gate:
    """What the reading admits: the seats to open, and the held with why."""

    seated: list[Seat]
    held: list[tuple[Seat, str]]
    remedies: list[str]
    notes: list[str]
    #: Under `--over-budget`, what the reading alone would have held.
    overridden: list[tuple[Seat, str]]


def _admit(
    seats: list[Seat], reading: dict, cap: int, over_budget: bool
) -> tuple[list[Seat], list[tuple[Seat, list[str]]]]:
    """Seat in order: free verbs on the machine checks alone, building ones up to
    the reading's slots. Under `over_budget` only the cap binds, and only
    building verbs."""
    verdict = reading.get("verdict")
    if verdict is None or over_budget:
        building = (reading.get("sessions") or {}).get("building", 0)
        b_slots, b_reasons, spent = max(0, cap - building), [], "cap"
        f_slots, f_reasons = None, []
    else:
        b_slots, b_reasons = (
            verdict["building"]["slots"],
            verdict["building"]["reasons"],
        )
        f_slots, f_reasons = verdict["free"]["slots"], verdict["free"]["reasons"]
        spent = "slots"
    seated, held, taken = [], [], 0
    for seat in seats:
        if seat.cls == FREE:
            if f_slots is None:
                seated.append(seat)
            else:
                held.append((seat, f_reasons))
        elif taken < b_slots:
            taken += 1
            seated.append(seat)
        else:
            held.append((seat, b_reasons or [spent]))
    return seated, held


def gate(
    seats: list[Seat],
    directory: Path,
    *,
    over_budget: bool = False,
    env: dict[str, str] | None = None,
    reader=None,
) -> Gate:
    """Read the probe once and seat what fits.

    A hold on the pool-lock wait is re-probed once before it stands: the probe
    takes one snapshot, and a build caught between two children reads as a
    waiter. That error leans toward refusing, and a second reading heals it.
    """
    reader = reader or probe
    env = os.environ if env is None else env
    reading = reader(seats, directory)
    has_reading = "verdict" in reading
    # The probe's own knob parse, so the cap means the same number in both
    # places; `physical` feeds only the memory reserve, which is unused here.
    knobs = machine_load.read_knobs(env, 0)
    cap = int((reading.get("knobs") or {}).get("cap", knobs.cap))
    notes = list(knobs.notes) if not has_reading else []
    if not has_reading:
        notes.append(
            f"no machine reading ({reading.get('error', 'unknown')}); "
            f"seating under the cap {cap} alone"
        )
    seated, held = _admit(seats, reading, cap, over_budget)
    if has_reading and not over_budget and any("wait" in why for _, why in held):
        # The first reading stands unless the second is one: its holds are
        # rendered from whichever reading produced them.
        second = reader(seats, directory)
        if "verdict" in second:
            reading = second
            seated, held = _admit(seats, reading, cap, over_budget)
    overridden: list[tuple[Seat, str]] = []
    if over_budget and has_reading:
        _, would = _admit(seats, reading, cap, False)
        kept = {id(seat) for seat, _ in held}
        overridden = [
            (seat, "; ".join(_reason(k, reading, cap) for k in why))
            for seat, why in would
            if id(seat) not in kept
        ]
    remedies: list[str] = []
    for _, why in held:
        for key in why:
            text = _remedy(key, reading)
            if text and text not in remedies:
                remedies.append(text)
    if any(seat.cls == BUILDING for seat, _ in held):
        remedies.append("any builder closing")
    return Gate(
        seated=seated,
        held=[
            (seat, "; ".join(_reason(k, reading, cap) for k in why))
            for seat, why in held
        ],
        remedies=remedies,
        notes=notes,
        overridden=overridden,
    )


def held_lines(result: Gate) -> list[str]:
    """The `held` lines, a repeated reason shortened to `same`, then what would
    free a slot."""
    lines, previous = [], None
    for seat, why in result.held:
        lines.append(f"held  {seat.shown}: {'same' if why == previous else why}")
        previous = why
    if result.held and result.remedies:
        lines.append(f"would free a slot: {'; '.join(result.remedies)}")
    lines += [
        f"over budget, seated anyway: {seat.shown}: {why}"
        for seat, why in result.overridden
    ]
    return lines


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="session_dispatch.py",
        description=(
            "Open an iTerm2 tab per session verb, in this window, and type each. "
            f"Separate several verbs with a bare `{VERB_SEPARATOR}`."
        ),
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the lines that would be typed, and do nothing else",
    )
    parser.add_argument(
        "--class",
        dest="cls",
        choices=(BUILDING, FREE),
        default=None,
        help="class every chore in the batch, instead of inspecting its body",
    )
    parser.add_argument(
        "--over-budget",
        action="store_true",
        help="seat the whole batch past the machine checks (the cap still binds)",
    )
    parser.add_argument("--state-dir", type=Path, default=None, help=argparse.SUPPRESS)
    parser.add_argument(
        "verb",
        nargs=argparse.REMAINDER,
        help=f"the session verb(s), separated by `{VERB_SEPARATOR}`",
    )
    args = parser.parse_args(argv)

    # REMAINDER swallows everything after the first positional, so a trailing
    # `--dry-run` never registers as the flag — `session_dispatch.py plan
    # --dry-run` would not dry-run. Today `validate` happens to reject the
    # stray word so nothing is typed, but that is luck rather than design, and
    # dry-run is the look-before-you-type affordance on a boundary that types
    # into a live shell. Catch it explicitly and say where the flag goes.
    flags = ("--dry-run", "-n", "--class", "--over-budget", "--state-dir")
    if stray := [word for word in args.verb if word.split("=")[0] in flags]:
        print(
            f"session-dispatch: {stray[0]} must come BEFORE the verb "
            "(argparse REMAINDER swallows anything after it)"
            + (
                "; `fleet go --over-budget` is the operator's to type in their "
                "own tab, and is not dispatched"
                if args.verb[:2] == ["fleet", "go"]
                else ""
            ),
            file=sys.stderr,
        )
        return 2

    # Every verb is validated BEFORE any tab is opened, so a batch with one bad
    # verb dispatches nothing rather than half of itself. A partial batch is the
    # bad outcome here: the caller cannot tell which tabs it got without reading
    # them, and re-running to fix the typo would double the ones that worked.
    try:
        groups = [validate(group) for group in split_verbs(args.verb)]
    except ValueError as exc:
        print(f"session-dispatch: {exc}", file=sys.stderr)
        return 2

    if args.dry_run:
        for words in groups:
            print(command_line(words))
        return 0

    # The machine gate, still before any tab. Unlike a malformed verb, a held
    # one is valid, so a partial batch is correct HERE: the held lines say
    # which, and re-dispatching them later cannot double a seated one.
    directory = args.state_dir or state_dir()
    seats = [classify(words, args.cls) for words in groups]
    admitted = gate(seats, directory, over_budget=args.over_budget)
    for note in admitted.notes:
        print(f"session-dispatch: {note}", file=sys.stderr)
    held = held_lines(admitted)
    if not admitted.seated:
        print("\n".join(held))
        return 0
    write_records(admitted.seated, directory)
    commands = [seat.command for seat in admitted.seated]
    shown = {seat.command: seat.shown for seat in admitted.seated}
    roles = [SEAT_VERBS.get(command.split()[0]) for command in commands]

    try:
        ttys = iterm_api.open_tabs(commands, roles=roles)
    except iterm_api.ItermUnavailable as exc:
        # Loud, and useful. This is what makes "best effort" an acceptable bar:
        # the operator loses the convenience, never the launch.
        print(f"session-dispatch: {exc}", file=sys.stderr)

        # Only the verbs NOT YET SENT go in the hand-run list. This used to name
        # the whole batch, which is wrong in the one case that matters: a driver
        # failure after k tabs were opened and typed left those k running, and an
        # operator following the instructions typed a second copy of each into a
        # new tab.
        #
        # `unfinished()` owns the arithmetic rather than this caller deriving it
        # from `len(exc.ttys)`, because the length alone is the WRONG question: a
        # tab the driver reached but could not type into also needs re-running,
        # and reading it as done dropped the command silently.
        pending = set(exc.unfinished(len(commands)))
        sent = [(n, command) for n, command in enumerate(commands) if n not in pending]
        remaining = [command for n, command in enumerate(commands) if n in pending]
        if sent:
            print(
                f"session-dispatch: {len(sent)} tab(s) were already opened and "
                "typed into — do NOT re-run these:",
                file=sys.stderr,
            )
            for n, command in sent:
                tty = exc.ttys[n] if n < len(exc.ttys) else None
                print(f"  {command} -> {tty or 'tty unknown'}", file=sys.stderr)
        if remaining:
            listing = "\n".join(f"  {command}" for command in remaining)
            print(
                f"session-dispatch: run {'these' if len(remaining) > 1 else 'this'} "
                f"by hand instead:\n\n{listing}\n",
                file=sys.stderr,
            )
        for line in held:
            print(line, file=sys.stderr)
        return 1

    # The confirmation the operator reads. One line per verb pairs it with the
    # tty its tab landed on, which is what makes a batch auditable rather than
    # merely finished; `open_tabs` keeps that list positional, so a `None` marks
    # the one tab whose tty could not be read instead of shifting the rest.
    for command, tty in zip(commands, ttys):
        print(f"{shown[command]} -> {tty or 'tty unknown'}")
    for line in held:
        print(line)

    # Every new tab gets the operator's own Attend mark, green until visited,
    # so a dispatched or resumed session is attended to rather than found by
    # scanning (operator, 2026-10-19). Best effort, like the roll-call: a lost
    # tint never turns a dispatch that worked into a non-zero exit.
    unmarked = [
        command
        for command, tty in zip(commands, ttys)
        if not (tty and iterm_api.mark_attention(tty))
    ]
    if unmarked:
        print(
            f"session-dispatch: could not mark green: {', '.join(unmarked)}",
            file=sys.stderr,
        )

    # The roll-call, best-effort and deliberately AFTER the per-verb lines. It is
    # the independent check that the tabs really exist — the ttys above come from
    # the call that made them, so they cannot disprove their own success, whereas
    # a fresh listing can. Losing the roll-call must never turn a dispatch that
    # worked into a non-zero exit, so its failure only annotates.
    try:
        print(f"sessions now open: {', '.join(iterm_api.session_names())}")
    except iterm_api.ItermUnavailable as exc:
        print(f"session-dispatch: tabs opened; roll-call unavailable ({exc})")
    return 0


def main() -> int:
    return run(sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
