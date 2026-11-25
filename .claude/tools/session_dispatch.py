#!/usr/bin/env python3
# cspell:word commenters
"""Open a new iTerm tab per session verb and type the verb into it.

A board session's dispatch arm (the scheduler's, or the curator's for an
architect spin-off): it names a task that is ready to start,
offers to start it, and on the operator's yes runs this. The ask-then-yes IS
the authorization, the same class as `fleet go` — for the scheduler that yes is
the start trigger (or, on "this wave only", each later wave's ask) — and the one
gate this tool adds is the machine's (below), never a second ask.

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

**A dispatched `fleet` opens no tab at all**: it runs `fleet_resume.py` in
process (`run_fleet`), so the roll-call prints into the dispatching session
and the sessions it resumes open as tabs beside it. The tab driver's
window-create fallback once put a dispatched `fleet go` in a window of its own.
The operator's own typed `fleet go` is the shell verb, unchanged.

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
(`machine_load.py`) is read once for the batch. What fits is admitted; the rest
is printed as `held` with the number that refused it, and a scheduler session
re-dispatches it when a slot opens. `--wait <minutes>` does that waiting in the
tool, re-reading the gate every `WAIT_POLL_S` until the batch fits or the time
runs out, so a full fleet costs one call rather than a turn per retry. A hold
on the total also names the **finished** sessions: an idle tab whose issue is
In Review or Done, holding a slot until it closes. Every slot, free or
building, spends one of `DS_FLEET_TOTAL`, the ceiling on live top-level
clients the probe applies
and nothing here overrides. `--over-budget` admits past every other check, as
the operator's word in their own tab. A probe with no reading (exit 3) never
halts the fleet: the batch is admitted under the total alone, and the dispatch
says why — unless the total itself is unset, which holds the whole batch and
names the variable. `fleet_resume.py` runs the same gate over its resume list,
which is why the gate is public here rather than inlined in `run`.

**The knobs are read at call time** from the runtime config (the profile the
manifest `.claude/shell/runtime-config.manifest.json` describes), not from the
calling shell, which holds whatever it was launched with: a long-lived
scheduler tab would otherwise refuse every dispatch at a total raised since.
A knob where the shell and the config disagree prints a note naming both; an
unreadable config falls back to the environment.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import time
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

#: The substrate override words, the same on every verb that takes one.
_SUBSTRATES = ("anthropic", "bedrock")

#: The daily-role verbs, each mapped to the iTerm role its tab will carry —
#: which places the tab (`iterm_api.insert_index`): the scheduler's rightmost,
#: the others just left of it. Every other verb's tab is a worker's.
DAILY_VERBS = {
    "plan": iterm_api.PLAN_ROLE,
    "curate": iterm_api.CURATE_ROLE,
    "analyze": iterm_api.ANALYZE_ROLE,
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
            # The shell refuses 0 at launch, as it does on `explore` below.
            if sub != "resume" and re.fullmatch(r"(?:eng-)?0+", rest[0], re.ASCII):
                raise ValueError("`task` needs a real issue number; 0 is not one")
            return ["task", sub, rest[0]]
        if len(args) != 1 or not _TAG.match(args[0]):
            raise ValueError("`task` takes one eng-### tag or number")
        if re.fullmatch(r"(?:eng-)?0+", args[0], re.ASCII):
            raise ValueError("`task` needs a real issue number; 0 is not one")
        return ["task", args[0]]

    if verb == "explore":
        # An issue number is REQUIRED (it names the worktree) and the `resume`
        # twin is retired, since `explore <n>` now resumes an existing session
        # itself — `explore 1196` lands in the `1196` worktree. The free-form
        # `explore <name>` is retired with it: a worktree is named for its
        # issue, never a word. Both forms pass: the bare `explore <n>` the
        # dispatch prose types, whose launcher reads the title's
        # `[Explore/<Substrate>]` token, and either substrate word in `task`'s
        # position, the override.
        word = [args[0]] if args and args[0] in _SUBSTRATES else []
        args = args[len(word) :]
        if args and args[0] == "resume":
            raise ValueError(
                "`explore resume` is retired — dispatch `explore <n>`, "
                "which resumes an existing session itself"
            )
        if len(args) != 1 or not _TAG.match(args[0]):
            raise ValueError(
                "`explore` takes [anthropic|bedrock] and one eng-### tag or number"
            )
        # `_TAG` admits `0` and `eng-0`; the shell refuses both, since 0 is not a
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

    if verb in DAILY_VERBS:
        # The daily-role verbs: the day names the session, so no number.
        if len(args) > 1 or (args and args[0] not in _SUBSTRATES):
            raise ValueError(f"`{verb}` takes nothing, or anthropic or bedrock")
        return [verb, *args]

    raise ValueError(f"unknown verb {verb!r}")


#: Separates one verb from the next in a batch dispatch.
#:
#: A separator rather than inference, deliberately. The verb names are a closed
#: set, so consecutive verbs look as if they could be split apart without one —
#: but `explore` once took a lowercase name, which matched `plan`, so
#: `explore plan` was genuinely ambiguous between one verb and two, and any
#: future worded argument would make it so again. On a boundary that types into a
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

#: Each verb's class. `architect`, `explore` and the daily-role verbs run no cargo
#: and no Docker and open no PR, so only the machine checks can refuse them; `fleet`
#: is free because each session it resumes meets this same gate inside
#: `fleet_resume.py` (and `run` never gates it: it runs in process, no tab).
#: `chore` is absent on purpose — it is inspected
#: (`chore_class`) — and a verb added later is building until this says
#: otherwise, the direction that errs toward refusing.
VERB_CLASS = {
    "architect": FREE,
    "curate": FREE,
    "explore": FREE,
    "fleet": FREE,
    "analyze": FREE,
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
class Slot:
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


def classify(words: list[str], override: str | None = None, fetch=None) -> Slot:
    """Class one validated verb. ``override`` is `--class`, which binds chores."""
    fetch = fetch or fetch_body
    command = command_line(words)
    if words[0] != "chore":
        cls = VERB_CLASS.get(words[0], BUILDING)
        return Slot(command, cls, "(free)" if cls == FREE else "")
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
    return Slot(command, override, label, record=number)


def state_dir() -> Path:
    """The base checkout's session-substrate directory, the probe's own."""
    return machine_load.default_state_dir()


def write_records(slots: list[Slot], directory: Path) -> None:
    """Record each admitted chore's class beside its substrate marker.

    The class alone, no pid: the launcher stamps `<class> <pid>` when its
    `claude` starts, reading the class from here, and a resume reads it back
    instead of re-inspecting. A record already holding this class is left
    alone, so re-dispatching a live chore keeps the pid its launcher stamped
    rather than dropping it from the probe's count. Best effort — a lost
    record costs a chore counted as building, never a dispatch.
    """
    for slot in slots:
        if slot.record is None:
            continue
        path = directory / f"{slot.record}.class"
        try:
            if path.read_text().split()[:1] == [slot.cls]:
                continue
        except OSError:
            pass
        try:
            directory.mkdir(parents=True, exist_ok=True)
            path.write_text(f"{slot.cls}\n")
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


#: The probe's knobs, read from the runtime config at call time. One per line:
#: the manifest check reads a `("NAME", value)` pair as a default.
FLEET_KNOBS = (
    "DS_FLEET_TOTAL",
    "DS_FLEET_CPU_LOAD_MAX",
    "DS_FLEET_BUILD_WAIT_S",
    "DS_FLEET_RESERVE_GB",
)

#: A `DS_*` name, and a value that is a plain literal once unquoted: a
#: number, a word, a path. Anything else (`$`, a backtick, a substitution) is
#: not read.
_DS_NAME = re.compile(r"DS_[A-Z0-9_]+")
_LITERAL = re.compile(r"[A-Za-z0-9._/:+-]*")
#: The builtins that assign (or clear) a variable in the current shell.
_DECLARERS = ("export", "typeset", "declare", "local", "readonly")


def runtime_config_path() -> Path:
    """The untracked runtime config: the zsh profile that sources `init.zsh`."""
    return Path(os.environ.get("ZDOTDIR") or Path.home()) / ".zshrc"


def _first_command(line: str) -> list[str]:
    """The words of the line's first command: quotes removed, a comment
    dropped, stopped at `;` `&` `|`. Empty on a line the lexer refuses."""
    lexer = shlex.shlex(line, posix=True, punctuation_chars=";&|()")
    lexer.whitespace_split = True
    lexer.commenters = "#"
    words: list[str] = []
    try:
        for word in lexer:
            if word and all(ch in ";&|()" for ch in word):
                break
            words.append(word)
    except ValueError:
        return []
    return words


def read_runtime_config(path: Path) -> dict[str, str] | None:
    """The literal `DS_*` values the profile leaves set, as zsh would, or
    `None` when it cannot be read.

    Only COLUMN-ZERO lines count: an indented assignment is presumed to sit in
    a function or conditional body, which a line scan cannot evaluate, so it is
    left to the environment rather than guessed at. On each line only the
    first command is read: `NAME=v`, or `export` / `typeset` / `declare` with
    any number of `NAME=v` words (flags skipped), while `unset NAME` clears
    one. A `NAME=v cmd` prefix sets the variable for that command alone, so it
    is skipped; a value that is not a plain literal is skipped too.
    """
    try:
        text = path.read_text(errors="replace")
    except OSError:
        return None
    values: dict[str, str] = {}
    for line in text.splitlines():
        if not line[:1].isalpha():
            continue
        words = _first_command(line)
        if not words:
            continue
        if words[0] == "unset":
            for name in words[1:]:
                values.pop(name, None)
            continue
        if words[0] in _DECLARERS:
            assigns = [w for w in words[1:] if not w.startswith("-")]
        elif all("=" in w for w in words):
            assigns = words
        else:
            continue  # a command, or a `NAME=v cmd` prefix
        for word in assigns:
            name, eq, value = word.partition("=")
            if eq and _DS_NAME.fullmatch(name) and _LITERAL.fullmatch(value):
                values[name] = value
    return values


def fleet_env(env, path: Path) -> tuple[dict[str, str], list[str]]:
    """The environment with the fleet knobs taken from the runtime config.

    A long-lived scheduler or curator tab holds the knobs it was launched
    with, so a value raised in the config since then never reaches it through
    the environment. The config wins for every knob it sets, and a knob where
    the two disagree gets a note, so the stale shell is visible. An unreadable
    config leaves the environment as it is, and says so.
    """
    merged = dict(env)
    config = read_runtime_config(path)
    if config is None:
        return merged, [
            f"runtime config {path} unreadable; fleet knobs from the environment"
        ]
    notes = []
    for name in FLEET_KNOBS:
        if name not in config:
            continue
        held = env.get(name)
        if held is not None and held != config[name]:
            notes.append(
                f"{name}: this shell holds {held}, the runtime config "
                f"{config[name]}; using {config[name]} (a stale tab, reopen it)"
            )
        merged[name] = config[name]
    return merged, notes


def probe(slots: list[Slot], directory: Path, env=None) -> dict:
    """One machine-load reading as the probe's JSON, the probe run under `env`
    (this process's environment when `None`).

    Anything that is not a reading — exit 3, a crash, a timeout, output that is
    not the JSON shape — comes back as the probe's own no-reading shape, so the
    caller has one test (`"verdict" in reading`) rather than three.
    """
    building = sum(slot.cls == BUILDING for slot in slots)
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
            env=env,
        )
        reading = json.loads(proc.stdout)
        if isinstance(reading, dict) and ("verdict" in reading or "error" in reading):
            return reading
        error = "the probe printed no reading"
    except (OSError, subprocess.SubprocessError, ValueError) as exc:
        error = f"{type(exc).__name__}: {exc}"
    return {"status": "no-reading", "exit": machine_load.NO_READING, "error": error}


def _reason(key: str, reading: dict, total: int) -> str:
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
    if key == "total":
        return (
            f"{sessions['live']} clients of the total {total}"
            if sessions
            else f"the total {total} (no reading counted the live sessions)"
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
    if key == "pressure":
        return f"kernel pressure {reading['pressure']}"
    if key == "load":
        return f"load {reading['load']['load1']:.1f} (limit {knobs['cpu_load_max']:g})"
    return key


def _remedy(key: str, reading: dict) -> str:
    knobs = reading.get("knobs") or {}
    return {
        "wait": "the longest wait falling under "
        + machine_load.duration(knobs.get("build_wait_s", 0)),
        "total": "any session closing",
        "memory": "memory headroom over the reserve",
        "pressure": "kernel pressure back to normal",
        "load": f"load falling under {knobs.get('cpu_load_max', 0):g}",
    }.get(key, "")


@dataclass
class Gate:
    """What the reading admits: the slots to open, and the held with why."""

    admitted: list[Slot]
    held: list[tuple[Slot, str]]
    remedies: list[str]
    notes: list[str]
    #: Under `--over-budget`, what the reading alone would have held.
    overridden: list[tuple[Slot, str]]
    #: `clients N of T (builders B, free F)` once the admitted are counted, or
    #: empty when no reading counted the live clients.
    clients: str = ""


def _admit(
    slots: list[Slot], reading: dict, total: int, over_budget: bool
) -> tuple[list[Slot], list[tuple[Slot, list[str]]]]:
    """Admit in order, every slot spending one of the total: free verbs on the
    machine checks, building ones also up to the reading's building slots.
    Under `over_budget` only the total binds."""
    verdict = reading.get("verdict")
    if verdict is None or over_budget:
        live = (reading.get("sessions") or {}).get("live", 0)
        room = max(0, total - live)
        b_slots, b_reasons, f_slots, f_reasons = room, [], room, []
        spent = "total"
    else:
        b_slots, b_reasons = (
            verdict["building"]["slots"],
            verdict["building"]["reasons"],
        )
        # The free verdict is the total's room whenever the machine admits.
        f_slots, f_reasons = verdict["free"]["slots"], verdict["free"]["reasons"]
        spent = "slots"
    admitted, held, taken, built = [], [], 0, 0
    for slot in slots:
        # The total first: a builder it holds is held on the total, not on
        # the building slots, and so is freed by any session closing. A
        # builder's own reasons are a superset of free's (they add the wait).
        if taken >= f_slots:
            why = b_reasons if slot.cls == BUILDING else f_reasons
            held.append((slot, why or ["total"]))
        elif slot.cls == BUILDING and built >= b_slots:
            held.append((slot, b_reasons or [spent]))
        else:
            taken += 1
            built += slot.cls == BUILDING
            admitted.append(slot)
    return admitted, held


def _clients(reading: dict, total: int, admitted: list[Slot]) -> str:
    sessions = reading.get("sessions")
    if not sessions:
        return ""
    building = sessions["building"] + sum(s.cls == BUILDING for s in admitted)
    free = sessions["free"] + sum(s.cls == FREE for s in admitted)
    return f"clients {building + free} of {total} (builders {building}, free {free})"


def gate(
    slots: list[Slot],
    directory: Path,
    *,
    over_budget: bool = False,
    env: dict[str, str] | None = None,
    reader=None,
) -> Gate:
    """Read the probe once and admit what fits.

    A hold on the pool-lock wait is re-probed once before it stands: the probe
    takes one snapshot, and a build caught between two children reads as a
    waiter. That error leans toward refusing, and a second reading heals it.

    With no `env`, the fleet knobs come from the runtime config at call time
    (`fleet_env`), and the probe runs under the result.
    """
    reader = reader or probe
    config_notes: list[str] = []
    if env is None:
        env, config_notes = fleet_env(os.environ, runtime_config_path())
    reading = reader(slots, directory, env=env)
    has_reading = "verdict" in reading
    if has_reading:
        total = int(reading["knobs"]["total"])
        notes = list(config_notes)
    else:
        # The probe's own knob parse, so the total means the same number in
        # both places; `physical` feeds only the memory reserve, unused here.
        try:
            knobs = machine_load.read_knobs(env, 0)
        except machine_load.NoReading as exc:
            # No total to admit under: hold the batch, naming the variable.
            return Gate(
                admitted=[],
                held=[(slot, str(exc)) for slot in slots],
                remedies=["the knob set in the runtime config"],
                notes=config_notes
                + [f"no machine reading ({reading.get('error', 'unknown')})"],
                overridden=[],
            )
        total = knobs.total
        notes = config_notes + list(knobs.notes)
        notes.append(
            f"no machine reading ({reading.get('error', 'unknown')}); "
            f"admitting under the total {total} alone"
        )
    admitted, held = _admit(slots, reading, total, over_budget)
    if has_reading and not over_budget and any("wait" in why for _, why in held):
        # The first reading stands unless the second is one: its holds are
        # rendered from whichever reading produced them.
        second = reader(slots, directory, env=env)
        if "verdict" in second:
            reading = second
            admitted, held = _admit(slots, reading, total, over_budget)
    overridden: list[tuple[Slot, str]] = []
    if over_budget and has_reading:
        _, would = _admit(slots, reading, total, False)
        kept = {id(slot) for slot, _ in held}
        overridden = [
            (slot, "; ".join(_reason(k, reading, total) for k in why))
            for slot, why in would
            if id(slot) not in kept
        ]
    remedies: list[str] = []
    for _, why in held:
        for key in why:
            text = _remedy(key, reading)
            if text and text not in remedies:
                remedies.append(text)
    held_building = any(slot.cls == BUILDING for slot, _ in held)
    if held_building and "any session closing" not in remedies:
        remedies.append("any builder closing")
    return Gate(
        admitted=admitted,
        held=[
            (slot, "; ".join(_reason(k, reading, total) for k in why))
            for slot, why in held
        ],
        remedies=remedies,
        notes=notes,
        overridden=overridden,
        clients=_clients(reading, total, admitted),
    )


def held_lines(result: Gate) -> list[str]:
    """The `held` lines, a repeated reason shortened to `same`, then what would
    free a slot."""
    lines, previous = [], None
    for slot, why in result.held:
        lines.append(f"held  {slot.shown}: {'same' if why == previous else why}")
        previous = why
    if result.held and result.remedies:
        lines.append(f"would free a slot: {'; '.join(result.remedies)}")
    lines += [
        f"over budget, admitted anyway: {slot.shown}: {why}"
        for slot, why in result.overridden
    ]
    return lines


#: An issue-keyed session name — the bare tag after an optional status glyph,
#: as `fleet_resume._TAG_RE` reads it (that module imports this one, so the
#: pattern is restated rather than imported).
_SESSION_TAG = re.compile(
    r"\A(\W*?)\s*(?:(?:eng|ceo|exp)-)?(\d+)\s*\Z", re.I | re.ASCII
)

#: A client mid-turn spins its title glyph (the quarter circles, or a braille
#: spinner); an idle one shows a static mark, or none once its client exits.
_BUSY_GLYPH = re.compile(r"[◐-◓⠀-⣿]")

#: The states in which a session's work is handed off: its tab can close.
HANDED_OFF_STATES = ("In Review", "Done")

#: How often `--wait` re-reads the gate.
WAIT_POLL_S = 30


def idle_tags(names: list[str]) -> list[str]:
    """The tags of issue-keyed sessions whose name shows no busy glyph."""
    tags = []
    for name in names:
        match = _SESSION_TAG.match(str(name).strip())
        if match and not _BUSY_GLYPH.search(match.group(1)):
            tags.append(str(int(match.group(2))))
    return tags


def finished_sessions(names=None, api_key: str | None = None, post=None) -> list[str]:
    """``<tag> (<state>)`` for each idle tab whose issue is In Review or Done:
    a finished session holding a slot of the total until its tab closes.

    Best effort, and empty on any failure: it only names what closing would
    free, and never changes what the gate admits."""
    try:
        names = iterm_api.session_names() if names is None else names
        tags = idle_tags(names)
        if api_key is None:
            api_key = os.environ.get("LINEAR_API_KEY", "")
        if not tags or not api_key:
            return []
        post = post or linear_issue._post
        variables = {f"i{n}": f"ENG-{tag}" for n, tag in enumerate(tags)}
        data = post(api_key, linear_issue._state_query(len(tags)), variables)
    except (iterm_api.ItermUnavailable, linear_issue.LinearIssueError, OSError):
        return []
    out = []
    for n, tag in enumerate(tags):
        state = ((data.get(f"i{n}") or {}).get("state") or {}).get("name")
        if state in HANDED_OFF_STATES:
            out.append(f"{tag} ({state})")
    return out


def _total_binds(result: Gate) -> bool:
    return "any session closing" in result.remedies


def wait_for_room(
    slots: list[Slot],
    directory: Path,
    *,
    minutes: float,
    over_budget: bool = False,
    gater=None,
    clock=None,
    sleep=None,
) -> Gate:
    """Re-read the gate until nothing is held or `minutes` pass, then return
    the last reading — the tool blocks here, so the caller spends one call on
    a full fleet rather than one turn per retry. A hold no session closing can
    heal (the total unset) returns at once."""
    gater = gater or gate
    clock = clock or time.monotonic
    sleep = sleep or time.sleep
    deadline = clock() + minutes * 60
    result = gater(slots, directory, over_budget=over_budget)
    announced = False
    while result.held and result.remedies and clock() < deadline:
        if result.remedies == ["the knob set in the runtime config"]:
            break
        if not announced:
            print(
                f"session-dispatch: waiting up to {minutes:g} min for a slot "
                f"({len(result.held)} held)",
                file=sys.stderr,
            )
            announced = True
        sleep(max(0.0, min(WAIT_POLL_S, deadline - clock())))
        result = gater(slots, directory, over_budget=over_budget)
    return result


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
        help="admit the whole batch past the machine checks (the total still binds)",
    )
    parser.add_argument(
        "--wait",
        type=float,
        default=0,
        metavar="MINUTES",
        help="block up to MINUTES until the held fit, re-reading the gate every "
        f"{WAIT_POLL_S}s, then dispatch what it admits",
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
    flags = ("--dry-run", "-n", "--class", "--over-budget", "--wait", "--state-dir")
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
    if args.wait < 0:
        print(
            "session-dispatch: --wait takes a positive number of minutes",
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

    # A `fleet` verb runs here, in process, never in a tab: the roll-call
    # belongs in the dispatching session, and the tab driver's window-create
    # fallback is what once opened it in a window of its own. It opens no
    # client, so it spends no slot of the gate below; each session it resumes
    # meets that gate inside `fleet_resume.py`. It runs after the tabs, so its
    # gate sees them.
    fleet = [words for words in groups if words[0] == "fleet"]
    groups = [words for words in groups if words[0] != "fleet"]
    if groups:
        rc = _dispatch_tabs(groups, args, by_hand=[command_line(w) for w in fleet])
        if rc:
            return rc
    for words in fleet:
        rc = run_fleet(words)
        if rc:
            return rc
    return 0


#: The resume tool the `fleet` shell verb runs; a dispatched `fleet` runs it
#: directly, with the same flags.
FLEET_RESUME = Path(__file__).resolve().parent / "fleet_resume.py"


def run_fleet(words: list[str]) -> int:
    """Run ``fleet_resume.py`` as a subprocess, its output streaming into the
    dispatching session — ``fleet go`` applies, a bare ``fleet`` shows the
    plan. The operator's own typed ``fleet go`` is the shell verb, unchanged."""
    cmd = [sys.executable, str(FLEET_RESUME)]
    if words[1:] == ["go"]:
        cmd.append("--apply")
    sys.stdout.flush()
    try:
        return subprocess.run(cmd, check=False).returncode
    except OSError as exc:
        print(
            f"session-dispatch: fleet resume did not start ({exc}); "
            f"run `{command_line(words)}` by hand",
            file=sys.stderr,
        )
        return 1


def _dispatch_tabs(
    groups: list[list[str]], args: argparse.Namespace, by_hand: list[str]
) -> int:
    """Gate, open and confirm the tabs. ``by_hand`` names the in-process verbs
    a driver failure leaves never run, so they join its hand-run list."""
    # The machine gate, still before any tab. Unlike a malformed verb, a held
    # one is valid, so a partial batch is correct HERE: the held lines say
    # which, and re-dispatching them later cannot double an admitted one.
    directory = args.state_dir or state_dir()
    slots = [classify(words, args.cls) for words in groups]
    if args.wait:
        gated = wait_for_room(
            slots, directory, minutes=args.wait, over_budget=args.over_budget
        )
    else:
        gated = gate(slots, directory, over_budget=args.over_budget)
    for note in gated.notes:
        print(f"session-dispatch: {note}", file=sys.stderr)
    held = held_lines(gated)
    if gated.held and _total_binds(gated):
        # The total is held by tabs, not clients' work: name the finished
        # sessions whose tabs closing would free a slot.
        finished = finished_sessions()
        held.append(
            "finished (idle tab, issue handed off): "
            + (", ".join(finished) if finished else "none found")
        )
    if not gated.admitted:
        print("\n".join(held))
        return 0
    write_records(gated.admitted, directory)
    commands = [slot.command for slot in gated.admitted]
    shown = {slot.command: slot.shown for slot in gated.admitted}
    roles = [DAILY_VERBS.get(command.split()[0]) for command in commands]

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
        remaining += by_hand
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
    if gated.clients:
        print(gated.clients)
    return 0


def main() -> int:
    return run(sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
