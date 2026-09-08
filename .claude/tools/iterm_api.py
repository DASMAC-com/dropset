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

#: How long to wait on the driver. Generous, because creating N tabs is N round
#: trips to a GUI app — but bounded, because a hung driver would otherwise hang
#: a session launcher, and the whole family's contract is "fatal never".
DRIVER_TIMEOUT_SECONDS = 60


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


def _call(request: dict) -> dict:
    """Run one request through the driver. Raises ``ItermUnavailable``."""
    reason = preflight()
    if reason is not None:
        raise ItermUnavailable(reason)

    interpreter = bundled_interpreter()
    try:
        completed = subprocess.run(
            [str(interpreter), str(Path(__file__).resolve()), "--_driver"],
            input=json.dumps(request),
            capture_output=True,
            text=True,
            check=False,
            timeout=DRIVER_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as exc:
        raise ItermUnavailable(
            f"the iTerm2 driver did not answer within {DRIVER_TIMEOUT_SECONDS}s"
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
    """
    return _call({"op": "open_window", "command": command}).get("ttys", [None])[0]


def open_tabs(commands: list[str]) -> list[str | None]:
    """Open one tab per command in the current window, typing each. Returns ttys.

    One driver round trip for the whole batch rather than one per tab: a
    per-command process would pay interpreter startup N times and interleave
    badly with the tabs it is creating. The returned list is positional, so a
    ``None`` marks the tab whose tty could not be read — never a silent gap.
    """
    if not commands:
        return []
    response = _call({"op": "open_tabs", "commands": list(commands)})
    ttys = list(response.get("ttys") or [])
    # Positional contract, defended here rather than trusted: a short list from
    # a future driver would otherwise silently shift every caller's tag/tty
    # pairing by one, which is exactly the class of bug that made the previous
    # AppleScript path report a clean summary over a total mark failure.
    ttys += [None] * (len(commands) - len(ttys))
    return ttys[: len(commands)]


# ---------------------------------------------------------------------------
# Driver half. Everything below runs under iTerm's own interpreter.
# ---------------------------------------------------------------------------


def _first_session(window):
    """The session to type into, from a just-created window.

    ``window.current_tab`` is None on a window this process only just created —
    that attribute reads the app's cached state, which has not caught up with a
    window the cache does not know exists. Measured on the first live run of the
    dispatcher. ``window.tabs`` IS populated on the returned object, so walk it
    and keep ``current_tab`` as the fallback for any iTerm version where the
    reverse holds.
    """
    for tab in getattr(window, "tabs", None) or []:
        sessions = getattr(tab, "sessions", None) or []
        if sessions:
            return sessions[0]
        if getattr(tab, "current_session", None) is not None:
            return tab.current_session
    tab = getattr(window, "current_tab", None)
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
        ttys = []
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
        result["ttys"] = ttys
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
