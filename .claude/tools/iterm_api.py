#!/usr/bin/env python3
"""Drive iTerm2 through its Python API. The one owner of iTerm automation.

Two callers: `session_dispatch.py` (open ONE new window and type a verb) and
`fleet_resume.py` (open one TAB per in-flight issue and type a resume verb into
each). Both used to reach for AppleScript; this retires it from the toolbox
entirely, which was the point of consolidating them here rather than letting the
second caller grow its own copy.

WHY A SEPARATE PROCESS. `iterm2` is not stdlib, and the repo's tools convention
is stdlib-only so nothing here needs a pip install. iTerm ships its own Python
environment with `iterm2` already in it, so this module stays importable from
ordinary `python3` and shells out to that interpreter — running THIS SAME FILE
in `--_driver` mode — for the handful of calls that need the library. The
convention holds and the operator installs nothing.

The wire format between the two halves is one JSON request on stdin and one
JSON response on stdout, which keeps the boundary explicit and lets the pure
half be tested without an iTerm anywhere in sight.

BEST EFFORT IS THE BAR. Every entry point reports failure as data rather than
raising, and every caller is expected to carry on and tell the operator what it
could not do. iTerm's API needs the API enabled in settings, needs macOS
Automation permission, and breaks on some iTerm upgrades; over SSH it does
nothing useful at all. None of that may be allowed to take down the tool that
was merely trying to open a tab.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

#: Where iTerm keeps the Python environments it ships. Each version directory is
#: a full interpreter; the newest one carrying `iterm2` is the one to use.
ITERM_ENV_ROOT = (
    Path.home()
    / "Library"
    / "Application Support"
    / "iTerm2"
    / "iterm2env"
    / "versions"
)

#: The API socket iTerm listens on. Its ABSENCE is the one failure diagnosable
#: precisely and cheaply, so it is checked before anything expensive: it means
#: the Python API is switched off in iTerm's settings, which is an operator fix
#: (Settings > General > Magic > Enable Python API) that no retry will clear.
ITERM_SOCKET = (
    Path.home() / "Library" / "Application Support" / "iTerm2" / "private" / "socket"
)

#: Base wait on the driver: connection setup plus one round trip. Bounded,
#: because a hung driver would otherwise hang a session launcher and the whole
#: family's contract is "fatal never".
DRIVER_TIMEOUT_SECONDS = 60

#: Added per tab on a batch op. A fixed budget that is generous for one tab is
#: not generous for twenty, and the failure it produces is the worst kind: the
#: driver is killed mid-batch with tabs already open and typed into, while the
#: caller is told the whole batch failed.
PER_TAB_TIMEOUT_SECONDS = 10


class ItermUnavailable(Exception):
    """iTerm automation cannot run here. Carries a one-line operator-facing why."""


def bundled_interpreter(root: Path | None = None) -> Path | None:
    """The newest iTerm-bundled interpreter that actually has ``iterm2``.

    Sorted by version TUPLE rather than lexically: iTerm keeps several versions
    side by side, and a string sort puts ``3.8.19`` above ``3.14.0``, which
    would pick a years-old interpreter whenever both are present. Measured on a
    machine carrying 3.7, 3.8, 3.10 and 3.14 at once.

    Presence is checked by looking for the package directory rather than by
    importing it, since importing would mean running the other interpreter — a
    subprocess per candidate, on a path that runs at every dispatch.
    """
    root = ITERM_ENV_ROOT if root is None else root
    if not root.is_dir():
        return None

    def version_key(path: Path) -> tuple:
        return tuple(
            int(chunk) if chunk.isdigit() else -1 for chunk in path.name.split(".")
        )

    for version_dir in sorted(root.iterdir(), key=version_key, reverse=True):
        if not version_dir.is_dir():
            continue
        interpreter = version_dir / "bin" / "python3"
        if not interpreter.exists():
            continue
        if any(version_dir.glob("lib/python*/site-packages/iterm2")):
            return interpreter
    return None


def preflight() -> str | None:
    """A one-line reason iTerm automation cannot work, or None. Cheap checks."""
    if sys.platform != "darwin":
        return "iTerm2 is macOS-only, and this is not macOS"
    if not ITERM_SOCKET.exists():
        return (
            "iTerm2's Python API socket is absent — enable it in Settings > "
            "General > Magic > Enable Python API, and make sure iTerm2 is running"
        )
    if bundled_interpreter() is None:
        return (
            "no iTerm2-bundled Python carrying the `iterm2` package under "
            f"{ITERM_ENV_ROOT}"
        )
    return None


def _call(request: dict, *, timeout: int | None = None) -> dict:
    """Run one request through the driver. Raises ``ItermUnavailable``.

    ``timeout`` defaults to :data:`DRIVER_TIMEOUT_SECONDS`. A batch op passes a
    larger one: the budget has to cover N round trips to a GUI app, and a fixed
    figure that is generous for one tab is not generous for twenty.
    """
    reason = preflight()
    if reason is not None:
        raise ItermUnavailable(reason)

    budget = DRIVER_TIMEOUT_SECONDS if timeout is None else timeout
    interpreter = bundled_interpreter()
    try:
        completed = subprocess.run(
            [str(interpreter), str(Path(__file__).resolve()), "--_driver"],
            input=json.dumps(request),
            capture_output=True,
            text=True,
            check=False,
            timeout=budget,
        )
    except subprocess.TimeoutExpired as exc:
        raise ItermUnavailable(
            f"the iTerm2 driver did not answer within {budget}s"
        ) from exc
    except OSError as exc:
        raise ItermUnavailable(f"cannot run the iTerm2 driver: {exc}") from exc

    if not completed.stdout.strip():
        detail = (completed.stderr or "").strip().splitlines()
        raise ItermUnavailable(
            f"the iTerm2 driver returned nothing ({detail[-1] if detail else 'no detail'})"
        )
    try:
        response = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise ItermUnavailable(f"unparseable iTerm2 driver response: {exc}") from exc

    if not response.get("ok"):
        raise ItermUnavailable(response.get("error") or "the iTerm2 API call failed")
    return response


def session_names() -> list[str]:
    """Every open iTerm session's name, across every window and tab.

    Session, not tab: the two are equivalent for the one-pane tabs these helpers
    create, but the distinction matters if a tab is ever split.
    """
    return list(_call({"op": "session_names"}).get("names") or [])


def open_window(command: str) -> str | None:
    """Open a NEW window and type ``command`` into it. Returns its tty, if any.

    A new window rather than a tab, deliberately: one iTerm window per session
    is how the operator talks to the fleet. This never falls back to the current
    session — typing a launch verb into a window already running something is
    worse than not dispatching at all.

    The tty is read defensively. A bare ``.get("ttys", [None])[0]`` applies its
    default only when the key is ABSENT, so a driver answering ``"ttys": []``
    would raise IndexError and one answering ``"ttys": null`` TypeError — either
    escaping as a raw traceback past `session_dispatch`, which catches only
    `ItermUnavailable` and is the code path whose entire job is to print the
    verb you can run by hand.
    """
    ttys = _call({"op": "open_window", "command": command}).get("ttys") or []
    return ttys[0] if ttys else None


def open_tabs(commands: list[str]) -> list[str | None]:
    """Open one tab per command in the current window, typing each. Returns ttys.

    One driver round trip for the whole batch rather than one per tab: a
    per-command process would pay interpreter startup N times and interleave
    badly with the tabs it is creating. The returned list is positional, so a
    ``None`` marks the tab whose tty could not be read — never a silent gap.
    """
    if not commands:
        return []
    response = _call(
        {"op": "open_tabs", "commands": list(commands)},
        # One round trip per tab against a GUI app, so the budget scales.
        timeout=DRIVER_TIMEOUT_SECONDS + PER_TAB_TIMEOUT_SECONDS * len(commands),
    )
    ttys = list(response.get("ttys") or [])
    # Positional contract, defended here rather than trusted. Note precisely
    # what the pad buys: it restores the LENGTH, so `zip` cannot drop trailing
    # tags. It cannot repair a gap in the MIDDLE — everything after such a gap
    # is still shifted, and the pad then lets `zip` consume it. The driver makes
    # that unreachable by appending None in place, so gaps are positional by
    # construction; this is a guard on the length invariant alone.
    ttys += [None] * (len(commands) - len(ttys))
    return ttys[: len(commands)]


# ---------------------------------------------------------------------------
# Driver half. Everything below runs under iTerm's own interpreter.
# ---------------------------------------------------------------------------


def _first_session(container):
    """The session to type into, given EITHER a Window or a Tab.

    Both shapes are handled because the two ops pass different ones:
    ``open_window`` hands over a Window, ``open_tabs`` a Tab. A Tab exposes
    ``.sessions`` / ``.current_session``; a Window exposes ``.tabs``.

    **Taking a Tab is not hypothetical generality — it is a fixed bug.** An
    earlier version inspected only the Window attributes, so when ``open_tabs``
    passed a Tab both lookups missed, this returned None for every tab, and the
    driver appended a null tty and moved on WITHOUT TYPING ANYTHING. `fleet
    --apply` opened one blank tab per in-flight issue and resumed none of them,
    while reporting ``opened: 0`` — visible, but only if someone read the
    summary. Confirmed live: a probe whose typed command would have created a
    marker file produced the tab and no marker.

    The two blocks are disjoint in practice — a Window exposes no
    ``.sessions``, a Tab no ``.tabs`` — so their relative order is not what
    makes this correct; handling BOTH shapes is. Stated plainly because the
    opposite claim invites a future reader to preserve a constraint that does
    not exist, and because reverting the block order would leave every test
    green.

    The order that IS load-bearing sits inside the Window branch: tabs come
    before ``current_tab``, since ``current_tab`` is None on a window this
    process only just created — that attribute reads the app's cached state,
    which has not caught up with a window the cache does not know exists. Also
    measured live, on the dispatcher's first run.
    """
    # Tab-shaped: answers directly.
    sessions = getattr(container, "sessions", None) or []
    if sessions:
        return sessions[0]
    current = getattr(container, "current_session", None)
    if current is not None:
        return current

    # Window-shaped: walk its tabs, with current_tab as the late fallback.
    for tab in getattr(container, "tabs", None) or []:
        sessions = getattr(tab, "sessions", None) or []
        if sessions:
            return sessions[0]
        current = getattr(tab, "current_session", None)
        if current is not None:
            return current
    tab = getattr(container, "current_tab", None)
    return getattr(tab, "current_session", None) if tab is not None else None


async def _driver_body(connection, request, result):  # pragma: no cover
    import iterm2

    op = request.get("op")

    if op == "session_names":
        app = await iterm2.async_get_app(connection)
        names = []
        for window in app.terminal_windows:
            for tab in window.tabs:
                for session in tab.sessions:
                    name = await session.async_get_variable("autoName")
                    if name:
                        names.append(name)
        result["names"] = names
        result["ok"] = True
        return

    if op == "open_window":
        window = await iterm2.Window.async_create(connection)
        if window is None:
            result["error"] = "iTerm2 refused to create a window"
            return
        session = _first_session(window)
        if session is None:
            result["error"] = "the new iTerm2 window came back with no session"
            return
        await session.async_send_text(request["command"] + "\n")
        result["ttys"] = [await session.async_get_variable("tty")]
        result["ok"] = True
        return

    if op == "open_tabs":
        app = await iterm2.async_get_app(connection)
        window = app.current_terminal_window
        if window is None:
            window = await iterm2.Window.async_create(connection)
        if window is None:
            result["error"] = "iTerm2 refused to create a window"
            return
        # Published into `result` BEFORE the loop, and mutated in place, so a
        # failure partway through a batch still reports the tabs already opened.
        # Otherwise the caller is told the whole batch failed and prints every
        # verb as "run these by hand" — which double-resumes the first k
        # sessions, since those tabs are open and running.
        ttys = []
        result["ttys"] = ttys
        for command in request["commands"]:
            tab = await window.async_create_tab()
            session = _first_session(tab) if tab is not None else None
            if session is None:
                # Positional: record the gap rather than dropping the entry, so
                # the caller can name which tab it could not reach.
                ttys.append(None)
                continue
            await session.async_send_text(command + "\n")
            ttys.append(await session.async_get_variable("tty"))
        result["ok"] = True
        return

    result["error"] = f"unknown op {op!r}"


def _driver() -> int:  # pragma: no cover - needs iTerm's own interpreter
    try:
        request = json.loads(sys.stdin.read() or "{}")
    except json.JSONDecodeError as exc:
        json.dump({"ok": False, "error": f"bad request: {exc}"}, sys.stdout)
        return 1

    result: dict = {"ok": False}

    try:
        import iterm2
    except ImportError as exc:
        json.dump({"ok": False, "error": f"cannot import iterm2: {exc}"}, sys.stdout)
        return 1

    async def body(connection):
        # Caught HERE rather than around `run_until_complete`, because that
        # wrapper prints its own traceback and calls `sys.exit(1)` — so an
        # `except Exception` outside it never runs (`SystemExit` derives from
        # `BaseException`) and the operator gets a raw iTerm traceback instead
        # of a diagnosis. Measured on the dispatcher's first live run.
        #
        # Broad on purpose: the API raises a zoo of connection, authentication
        # and protocol errors that are not a stable surface across iTerm
        # versions, and the recovery is identical for every one of them.
        try:
            await _driver_body(connection, request, result)
        except Exception as exc:
            result["ok"] = False
            result["error"] = f"iTerm2 API call failed: {exc}"

    try:
        iterm2.run_until_complete(body)
    except SystemExit:
        if not result.get("error"):
            result["error"] = (
                "the iTerm2 API connection failed — if macOS has not been told "
                "to allow it, grant Automation access in System Settings > "
                "Privacy & Security > Automation"
            )

    json.dump(result, sys.stdout)
    return 0 if result.get("ok") else 1


if __name__ == "__main__":
    if "--_driver" in sys.argv[1:]:
        sys.exit(_driver())
    print(__doc__.strip().splitlines()[0], file=sys.stderr)
    print("This module is a library; it has no standalone CLI.", file=sys.stderr)
    sys.exit(2)
