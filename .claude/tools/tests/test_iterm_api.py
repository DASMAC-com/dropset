#!/usr/bin/env python3
"""Unit tests for `iterm_api.py` (stdlib unittest).

The pure half: interpreter discovery, the response contract of `_call`, the
positional guarantee `open_tabs` makes to its callers, and `_first_session`'s
handling of a freshly created window. The driver half needs a live iTerm2 and is
deliberately not faked — a mock of the API would only assert that this module's
idea of the API is self-consistent, which is the one thing that cannot fail in a
useful way.
"""

from __future__ import annotations

import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

import iterm_api


class InterpreterDiscovery(unittest.TestCase):
    def _env(self, root: Path, version: str, *, with_pkg: bool, py="3.14"):
        base = root / version
        (base / "bin").mkdir(parents=True)
        (base / "bin" / "python3").write_text("#!/bin/sh\n", encoding="utf-8")
        site = base / "lib" / f"python{py}" / "site-packages"
        site.mkdir(parents=True)
        if with_pkg:
            (site / "iterm2").mkdir()

    def test_absent_root_is_none(self):
        with TemporaryDirectory() as tmp:
            self.assertIsNone(iterm_api.bundled_interpreter(Path(tmp) / "nope"))

    def test_picks_the_only_env_that_has_the_package(self):
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._env(root, "3.8.19", with_pkg=False, py="3.8")
            self._env(root, "3.14.0", with_pkg=True, py="3.14")
            found = iterm_api.bundled_interpreter(root)
            self.assertIsNotNone(found)
            self.assertIn("3.14.0", str(found))

    def test_versions_sort_numerically_not_lexically(self):
        # The bug this guards: a string sort puts "3.8.19" above "3.14.0", so a
        # lexical pick lands on a years-old interpreter whenever both exist —
        # which is the actual layout on the operator's machine (3.7, 3.8, 3.10
        # and 3.14 side by side).
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._env(root, "3.7.17", with_pkg=True, py="3.7")
            self._env(root, "3.8.19", with_pkg=True, py="3.8")
            self._env(root, "3.10.19", with_pkg=True, py="3.10")
            self._env(root, "3.14.0", with_pkg=True, py="3.14")
            self.assertIn("3.14.0", str(iterm_api.bundled_interpreter(root)))

    def test_env_without_the_package_is_skipped(self):
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._env(root, "3.14.0", with_pkg=False)
            self.assertIsNone(iterm_api.bundled_interpreter(root))

    def test_a_stray_file_in_the_root_is_ignored(self):
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "README").write_text("hi\n", encoding="utf-8")
            self._env(root, "3.14.0", with_pkg=True)
            self.assertIsNotNone(iterm_api.bundled_interpreter(root))

    def test_a_non_numeric_version_directory_does_not_crash_the_sort(self):
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._env(root, "wip", with_pkg=False, py="3.9")
            self._env(root, "3.14.0", with_pkg=True, py="3.14")
            self.assertIn("3.14.0", str(iterm_api.bundled_interpreter(root)))


class DriverStub:
    """Fakes the subprocess boundary and RECORDS what crossed it.

    A mixin rather than a base `TestCase`: `OpenTabsPositional` used to derive
    from `CallContract` to reuse this, which made unittest collect and re-run
    every parent case under the subclass too — inflating the suite count and
    reporting any parent failure twice.

    `sent` is the point. An earlier version discarded argv and stdin entirely,
    so nothing asserted WHAT `_call` sends: renaming the request's `op` from
    `session_names` to anything else left all six response-parsing cases green.
    That is a test passing whether or not its subject is right.
    """

    def _stub(self, *, stdout, stderr="", preflight=None, returncode=0, raises=None):
        self.addCleanup(setattr, iterm_api, "preflight", iterm_api.preflight)
        self.addCleanup(
            setattr, iterm_api, "bundled_interpreter", iterm_api.bundled_interpreter
        )
        self.addCleanup(setattr, iterm_api, "subprocess", iterm_api.subprocess)
        iterm_api.preflight = lambda: preflight
        iterm_api.bundled_interpreter = lambda root=None: Path("/bin/true")

        self.sent = {}
        completed = type("Completed", (), {})()
        completed.stdout = stdout
        completed.stderr = stderr
        completed.returncode = returncode

        real_timeout_expired = iterm_api.subprocess.TimeoutExpired

        def run(argv, **kwargs):
            self.sent["argv"] = argv
            self.sent["input"] = kwargs.get("input")
            self.sent["timeout"] = kwargs.get("timeout")
            if raises is not None:
                raise raises
            return completed

        stub = type("Subprocess", (), {})()
        stub.run = run
        stub.TimeoutExpired = real_timeout_expired
        iterm_api.subprocess = stub

    def sent_request(self):
        return json.loads(self.sent["input"])


class CallContract(DriverStub, unittest.TestCase):
    """`_call` turns every failure into `ItermUnavailable` with a reason."""

    def test_a_failed_preflight_never_spawns_the_driver(self):
        self._stub(stdout="", preflight="the API is off")
        with self.assertRaises(iterm_api.ItermUnavailable) as caught:
            iterm_api.session_names()
        self.assertIn("the API is off", str(caught.exception))

    def test_an_empty_driver_response_is_a_named_failure(self):
        # The shape a crashed driver produces. Reporting "returned nothing" beats
        # a JSONDecodeError traceback the operator cannot act on.
        self._stub(stdout="", stderr="Traceback...\nboom\n")
        with self.assertRaises(iterm_api.ItermUnavailable) as caught:
            iterm_api.session_names()
        self.assertIn("returned nothing", str(caught.exception))
        self.assertIn("boom", str(caught.exception))

    def test_unparseable_output_is_a_named_failure(self):
        self._stub(stdout="not json")
        with self.assertRaises(iterm_api.ItermUnavailable) as caught:
            iterm_api.session_names()
        self.assertIn("unparseable", str(caught.exception))

    def test_an_error_response_carries_the_drivers_own_reason(self):
        self._stub(stdout=json.dumps({"ok": False, "error": "no window"}))
        with self.assertRaises(iterm_api.ItermUnavailable) as caught:
            iterm_api.session_names()
        self.assertEqual(str(caught.exception), "no window")

    def test_session_names_returns_the_list(self):
        self._stub(stdout=json.dumps({"ok": True, "names": ["◐ eng-914", "Default"]}))
        self.assertEqual(iterm_api.session_names(), ["◐ eng-914", "Default"])

    def test_open_window_returns_the_tty(self):
        self._stub(stdout=json.dumps({"ok": True, "ttys": ["/dev/ttys004"]}))
        self.assertEqual(iterm_api.open_window("task 1"), "/dev/ttys004")

    def test_an_empty_ttys_list_is_none_not_an_IndexError(self):
        # `.get("ttys", [None])[0]` applies its default only when the key is
        # ABSENT, so `"ttys": []` used to raise IndexError — escaping past
        # session_dispatch, which catches only ItermUnavailable and whose whole
        # job on that path is printing the verb you can run by hand.
        self._stub(stdout=json.dumps({"ok": True, "ttys": []}))
        self.assertIsNone(iterm_api.open_window("task 1"))

    def test_a_null_ttys_is_none_not_a_TypeError(self):
        self._stub(stdout=json.dumps({"ok": True, "ttys": None}))
        self.assertIsNone(iterm_api.open_window("task 1"))

    # --- what _call SENDS, not just what it parses ---------------------------

    def test_the_request_names_the_op_and_rides_stdin(self):
        # stdin, not argv: a request on the command line would be visible in
        # `ps` to every process on the machine.
        self._stub(stdout=json.dumps({"ok": True, "names": []}))
        iterm_api.session_names()
        self.assertEqual(self.sent_request(), {"op": "session_names"})
        self.assertIn("--_driver", self.sent["argv"])
        self.assertNotIn("session_names", " ".join(self.sent["argv"]))

    def test_open_window_sends_its_command_verbatim(self):
        self._stub(stdout=json.dumps({"ok": True, "ttys": ["/dev/a"]}))
        iterm_api.open_window("task local 1234")
        self.assertEqual(
            self.sent_request(),
            {"op": "open_window", "command": "task local 1234"},
        )

    def test_open_tabs_sends_every_command_in_order(self):
        self._stub(stdout=json.dumps({"ok": True, "ttys": ["/dev/a", "/dev/b"]}))
        iterm_api.open_tabs(["task resume 1", "task resume 2"])
        self.assertEqual(
            self.sent_request(),
            {"op": "open_tabs", "commands": ["task resume 1", "task resume 2"]},
        )

    def test_the_batch_timeout_scales_with_the_command_count(self):
        # A fixed budget generous for one tab is not generous for twenty, and
        # the failure it produces is the bad kind: the driver is killed
        # mid-batch with tabs already open and typed into.
        self._stub(stdout=json.dumps({"ok": True, "ttys": ["/dev/a"]}))
        iterm_api.open_window("task 1")
        one_shot = self.sent["timeout"]

        self._stub(stdout=json.dumps({"ok": True, "ttys": ["/dev/a"] * 5}))
        iterm_api.open_tabs(["a", "b", "c", "d", "e"])
        self.assertGreater(self.sent["timeout"], one_shot)

    # --- failure branches of the subprocess call -----------------------------

    def test_a_timeout_is_a_named_failure(self):
        self._stub(
            stdout="",
            raises=iterm_api.subprocess.TimeoutExpired(cmd="driver", timeout=60),
        )
        with self.assertRaises(iterm_api.ItermUnavailable) as caught:
            iterm_api.session_names()
        self.assertIn("did not answer", str(caught.exception))

    def test_a_driver_that_cannot_start_is_a_named_failure(self):
        self._stub(stdout="", raises=OSError("no such interpreter"))
        with self.assertRaises(iterm_api.ItermUnavailable) as caught:
            iterm_api.session_names()
        self.assertIn("cannot run", str(caught.exception))

    def test_a_nonzero_exit_with_parseable_output_is_still_honored(self):
        # The driver reports failure in its JSON, not via exit status, so a
        # non-zero exit carrying a well-formed error must surface that error
        # rather than a generic one.
        self._stub(
            stdout=json.dumps({"ok": False, "error": "iTerm2 refused"}),
            returncode=1,
        )
        with self.assertRaises(iterm_api.ItermUnavailable) as caught:
            iterm_api.session_names()
        self.assertEqual(str(caught.exception), "iTerm2 refused")


class OpenTabsPositional(DriverStub, unittest.TestCase):
    """`open_tabs` promises one entry per command, in order."""

    def test_no_commands_makes_no_call_at_all(self):
        # Guarded because the driver would otherwise be spawned to do nothing,
        # and on a machine with no iTerm that turns a no-op into a failure.
        self.addCleanup(setattr, iterm_api, "_call", iterm_api._call)
        iterm_api._call = lambda request: self.fail("should not have called")
        self.assertEqual(iterm_api.open_tabs([]), [])

    def test_ttys_come_back_in_order(self):
        self._stub(stdout=json.dumps({"ok": True, "ttys": ["/dev/a", "/dev/b"]}))
        self.assertEqual(iterm_api.open_tabs(["x", "y"]), ["/dev/a", "/dev/b"])

    def test_a_short_response_is_padded_not_shifted(self):
        # The failure this prevents: a caller zips tags against ttys, so a
        # missing entry would silently pair every later tag with the wrong tty —
        # the same class of bug that once let a total mark failure report a
        # clean summary.
        self._stub(stdout=json.dumps({"ok": True, "ttys": ["/dev/a"]}))
        self.assertEqual(iterm_api.open_tabs(["x", "y", "z"]), ["/dev/a", None, None])

    def test_a_null_entry_is_preserved_as_a_gap(self):
        self._stub(stdout=json.dumps({"ok": True, "ttys": ["/dev/a", None]}))
        self.assertEqual(iterm_api.open_tabs(["x", "y"]), ["/dev/a", None])

    def test_an_over_long_response_is_truncated(self):
        self._stub(
            stdout=json.dumps({"ok": True, "ttys": ["/dev/a", "/dev/b", "/dev/c"]})
        )
        self.assertEqual(iterm_api.open_tabs(["x"]), ["/dev/a"])


class FirstSession(unittest.TestCase):
    """`_first_session` — the fix for a real failure on the first live run.

    `window.current_tab` is None on a window this process only just created, so
    the obvious `window.current_tab.current_session` raises `AttributeError`
    inside the async body. These fakes stand in for the shapes that matter.
    """

    class _Session:
        pass

    def _tab(self, *, sessions=None, current=None):
        tab = type("Tab", (), {})()
        tab.sessions = sessions
        tab.current_session = current
        return tab

    def _window(self, *, tabs=None, current_tab=None):
        window = type("Window", (), {})()
        window.tabs = tabs
        window.current_tab = current_tab
        return window

    def test_prefers_the_first_tabs_first_session(self):
        wanted = self._Session()
        window = self._window(tabs=[self._tab(sessions=[wanted])])
        self.assertIs(iterm_api._first_session(window), wanted)

    def test_handles_a_freshly_created_window_with_no_current_tab(self):
        # The exact shape that broke the first live dispatch.
        wanted = self._Session()
        window = self._window(tabs=[self._tab(sessions=[wanted])], current_tab=None)
        self.assertIs(iterm_api._first_session(window), wanted)

    def test_falls_back_to_current_tab_when_tabs_is_empty(self):
        wanted = self._Session()
        window = self._window(tabs=[], current_tab=self._tab(current=wanted))
        self.assertIs(iterm_api._first_session(window), wanted)

    def test_falls_back_to_a_tabs_current_session(self):
        wanted = self._Session()
        window = self._window(tabs=[self._tab(sessions=[], current=wanted)])
        self.assertIs(iterm_api._first_session(window), wanted)

    def test_no_session_anywhere_is_none_not_an_exception(self):
        # Reported as a failure line, never as a traceback in the operator's
        # face — the whole point of the fallback path.
        window = self._window(tabs=None, current_tab=None)
        self.assertIsNone(iterm_api._first_session(window))

    # --- the Tab-shaped caller: this is the blocking-bug regression guard ----

    def test_a_TAB_resolves_directly(self):
        # THE BUG THIS PINS. `open_tabs` passes a Tab, not a Window. An earlier
        # version inspected only `.tabs` / `.current_tab`, which a Tab has
        # neither of, so this returned None for every tab, the driver appended a
        # null tty and moved on WITHOUT TYPING ANYTHING — `fleet --apply` opened
        # one blank tab per in-flight issue and resumed none of them. Confirmed
        # live: a probe whose typed command would have created a marker file
        # produced the tab and no marker.
        wanted = self._Session()
        tab = self._tab(sessions=[wanted])
        self.assertIs(iterm_api._first_session(tab), wanted)

    def test_a_TAB_with_only_current_session_resolves(self):
        wanted = self._Session()
        tab = self._tab(sessions=[], current=wanted)
        self.assertIs(iterm_api._first_session(tab), wanted)

    def test_a_window_has_no_own_session_and_descends_into_its_tabs(self):
        # The two shapes are disjoint: a Window exposes no `.sessions`, so
        # adding the Tab-shaped block in front must not divert a Window away
        # from the tabs walk that was already working and is live-verified.
        wanted = self._Session()
        window = self._window(tabs=[self._tab(sessions=[wanted])], current_tab=None)
        self.assertIsNone(getattr(window, "sessions", None))
        self.assertIs(iterm_api._first_session(window), wanted)

    def test_a_tab_whose_sessions_is_None_rather_than_empty(self):
        # `or []` guards it, but the fakes only ever handed None to a Window.
        wanted = self._Session()
        tab = self._tab(sessions=None, current=wanted)
        self.assertIs(iterm_api._first_session(tab), wanted)


if __name__ == "__main__":
    unittest.main()
