"""Tests for ``flow_shapes.py`` — runs of judgment-free turns ranked by the
prefix replay one tool run would save — and its wiring into
``session_metrics.py`` and the digest."""

from __future__ import annotations

import contextlib
import io
import json
import os
import pathlib
import tempfile
import unittest
from unittest import mock

import flow_shapes as fs
import session_metrics as sm


class Transcript:
    """Builds main-transcript records: one assistant message per turn, its
    results answered by a user record, each turn's prefix growing."""

    def __init__(self, prefix: int = 20_000, step: int = 1_000) -> None:
        self.records: list[dict] = []
        self.prefix = prefix
        self.step = step
        self._n = 0

    def _stamp(self) -> str:
        return f"2026-11-24T10:{self._n // 60:02d}:{self._n % 60:02d}.000Z"

    def turn(self, *calls: tuple[str, dict], text: str = "") -> Transcript:
        self._n += 1
        mid = f"msg_{self._n}"
        usage = {"input_tokens": 0, "cache_read_input_tokens": self.prefix}
        uses = [
            {"type": "tool_use", "id": f"t{self._n}_{i}", "name": name, "input": args}
            for i, (name, args) in enumerate(calls)
        ]
        content = ([{"type": "text", "text": text}] if text else []) + uses
        self.records.append(
            {
                "type": "assistant",
                "timestamp": self._stamp(),
                "message": {
                    "id": mid,
                    "role": "assistant",
                    "content": content,
                    "usage": usage,
                },
            }
        )
        if uses:
            self.records.append(
                {
                    "type": "user",
                    "timestamp": self._stamp(),
                    "message": {
                        "role": "user",
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": u["id"],
                                "content": "ok",
                            }
                            for u in uses
                        ],
                    },
                }
            )
        self.prefix += self.step
        return self

    def bash(self, command: str) -> Transcript:
        return self.turn(("Bash", {"command": command}))

    def mcp(self, name: str) -> Transcript:
        return self.turn((f"mcp__{name}", {}))

    def typed(self, text: str) -> Transcript:
        self._n += 1
        self.records.append(
            {
                "type": "user",
                "timestamp": self._stamp(),
                "message": {"role": "user", "content": text},
            }
        )
        return self

    def runs(self, since=None, until=None) -> list[list[fs.Step]]:
        tracker = fs.FlowTracker(sm.bash_signature, since, until)
        for rec in self.records:
            tracker.observe(rec, sm.utc_stamp(rec.get("timestamp")))
        return tracker.finish()

    def jsonl(self) -> str:
        return "".join(json.dumps(r) + "\n" for r in self.records)


# The init-pr bootstrap before it became one tool run: a dozen deterministic
# acts in a row.
BOOTSTRAP = [
    ("bash", "gh auth status"),
    ("bash", "gh pr list --head eng-1 --state merged"),
    ("bash", "git fetch origin main"),
    ("bash", "git branch -m worktree-eng-1 eng-1"),
    ("bash", "git rebase origin/main"),
    ("bash", "pnpm --dir frontend install"),
    ("bash", "git commit --allow-empty -F /tmp/msg"),
    ("bash", "git push -u origin eng-1"),
    ("bash", "gh pr create --draft --title x"),
    ("bash", "gh api graphql -f query=mute"),
    ("mcp", "linear__save_issue"),
    ("mcp", "linear__get_issue"),
]
# The post-merge notification dismissal before the queue watch absorbed it.
DISMISSAL = [
    ("mcp", "github__list_notifications"),
    ("mcp", "github__dismiss_notification"),
]


def act(t: Transcript, kind: str, arg: str) -> None:
    t.bash(arg) if kind == "bash" else t.mcp(arg)


def worked_session(branch: str = "eng-1", notifications: int = 1) -> Transcript:
    """The worked instances in one session. ``branch`` varies the bootstrap's
    arguments between sessions; ``notifications`` is how many the merged PR
    had, each dismissed by id."""
    t = Transcript()
    t.typed("/init-pr")
    for kind, arg in BOOTSTRAP:
        act(t, kind, arg.replace("eng-1", branch))
    t.turn(("Read", {"file_path": "/tmp/spec.md"}))
    # Noise: a status-then-diff pair, between edits.
    t.bash("git status").bash("git diff --stat")
    t.turn(("Edit", {"file_path": "/x.py"}))
    t.bash("git status").bash("git diff --stat")
    t.turn(text="the PR merged")
    t.typed("close it out")
    t.mcp(DISMISSAL[0][1])
    for i in range(notifications):
        t.turn((f"mcp__{DISMISSAL[1][1]}", {"threadID": f"{branch}-{i}"}))
    t.turn(text="done")
    return t


class WorkedInstances(unittest.TestCase):
    def test_the_bootstrap_ranks_first_and_the_dismissal_pair_is_found(self):
        shapes = fs.rank([worked_session().runs(), worked_session().runs()])
        top = shapes[0]
        self.assertEqual(len(top.steps), len(BOOTSTRAP))
        self.assertEqual(top.steps[0], "gh auth status")
        self.assertEqual(top.steps[-1], "linear__get_issue")
        self.assertEqual((top.occurrences, len(top.sessions)), (2, 2))
        # A tool run leaves one turn per occurrence.
        self.assertEqual(top.turns_eliminated, 2 * (len(BOOTSTRAP) - 1))
        labels = [s.steps for s in shapes]
        self.assertIn(
            ("github__list_notifications", "github__dismiss_notification"), labels
        )

    def test_the_bootstrap_groups_by_shape_across_differing_arguments(self):
        shapes = fs.rank(
            [worked_session("eng-1").runs(), worked_session("eng-2").runs()]
        )
        self.assertEqual(len(shapes[0].steps), len(BOOTSTRAP))
        self.assertEqual(len(shapes[0].sessions), 2)

    def test_dismissing_each_notification_folds_into_one_loop_step(self):
        # Each dismissal's id came from the listing: mechanical, not a survey.
        shapes = fs.rank(
            [
                worked_session(notifications=3).runs(),
                worked_session(notifications=2).runs(),
            ]
        )
        (pair,) = [s for s in shapes if s.steps[0] == "github__list_notifications"]
        self.assertEqual(pair.steps[1], "github__dismiss_notification" + fs.LOOP)
        self.assertEqual((pair.turns, pair.turns_eliminated), (7, 5))

    def test_a_shape_held_twice_in_a_longer_one_is_still_subsumed(self):
        t = Transcript()
        for _ in range(2):
            t.typed("go")
            for c in ("git fetch", "git push", "git log", "git fetch", "git push"):
                t.bash(c)
        shapes = fs.rank([t.runs()])
        self.assertEqual(
            [s.steps for s in shapes],
            [("git fetch", "git push", "git log", "git fetch", "git push")],
        )

    def test_a_window_inside_a_longer_shape_is_subsumed(self):
        shapes = fs.rank([worked_session().runs(), worked_session().runs()])
        bootstrap = shapes[0].steps
        for s in shapes[1:]:
            self.assertFalse(
                fs._contains(s.steps, bootstrap), f"{s.steps} is inside the bootstrap"
            )

    def test_replay_saved_is_every_prefix_but_each_runs_first(self):
        t = Transcript(prefix=100, step=10)
        t.typed("go").bash("git fetch").bash("git push")
        t.typed("again").bash("git fetch").bash("git push")
        (shape,) = fs.rank([t.runs()])
        # Run 1 paid 100 then 110; run 2 paid 120 then 130: the tool keeps the
        # first turn of each, so 110 + 130 is saved.
        self.assertEqual(shape.replay_tokens, 240)
        self.assertEqual((shape.turns, shape.turns_eliminated), (4, 2))


class Boundaries(unittest.TestCase):
    def test_a_result_between_two_records_of_one_message_ends_no_turn(self):
        # Streamed execution: call A's result lands before call B's record.
        t = Transcript()
        for _ in range(2):
            t.typed("go").turn(
                ("Bash", {"command": "git fetch"}), ("Bash", {"command": "git push"})
            )
            first, results = t.records[-2], t.records[-1]
            a, b = first["message"]["content"]
            first["message"]["content"] = [a]
            second = json.loads(json.dumps(first))
            second["message"]["content"] = [a, b]  # the split repeats `a`
            t.records[-2:] = [first, results, second]
            t.bash("git log")
        (shape,) = fs.rank([t.runs()])
        self.assertEqual(shape.steps, ("git fetch + git push", "git log"))
        self.assertEqual(shape.turns, 4)

    def test_judgment_tools_prose_and_the_operator_end_a_run(self):
        t = Transcript()
        t.typed("a").bash("git fetch").turn(("Edit", {})).bash("git push")
        t.typed("b").bash("git fetch").turn(text="thinking aloud").bash("git push")
        t.typed("c").bash("git fetch").typed("wait").bash("git push")
        self.assertEqual(fs.rank([t.runs()]), [])

    def test_a_tool_search_neither_extends_nor_ends_a_run(self):
        t = Transcript()
        for _ in range(2):
            t.typed("go").bash("git fetch").turn(("ToolSearch", {})).bash("git push")
        (shape,) = fs.rank([t.runs()])
        self.assertEqual(shape.steps, ("git fetch", "git push"))

    def test_parallel_calls_are_one_step_and_split_records_one_turn(self):
        t = Transcript()
        t.typed("go").turn(
            ("Bash", {"command": "git fetch"}), ("mcp__linear__get_issue", {})
        )
        # The same message split across two records, the tool_use repeated.
        split = json.loads(json.dumps(t.records[-2]))
        t.records.insert(-1, split)
        t.bash("git push")
        t.typed("again").turn(
            ("mcp__linear__get_issue", {}), ("Bash", {"command": "git fetch"})
        )
        t.bash("git push")
        (shape,) = fs.rank([t.runs()])
        self.assertEqual(shape.steps, ("git fetch + linear__get_issue", "git push"))
        self.assertEqual(shape.turns, 4)

    def test_a_poll_folds_into_one_loop_step_whatever_its_length(self):
        t = Transcript()
        t.typed("go")
        for _ in range(3):
            t.bash("gh pr checks 1")
        t.bash("gh pr merge")
        t.typed("again")
        for _ in range(5):
            t.bash("gh pr checks 1")
        t.bash("gh pr merge")
        (shape,) = fs.rank([t.runs()])
        self.assertEqual(shape.steps, ("gh pr checks" + fs.LOOP, "gh pr merge"))
        self.assertEqual((shape.turns, shape.turns_eliminated), (10, 8))

    def test_a_search_survey_is_judgment_not_a_loop(self):
        # The same shape with new arguments: each chosen from the last hit.
        t = Transcript()
        for _ in range(2):
            t.typed("go").bash("grep -n 'Foo(' a.py").bash("grep -n 'Bar(' b.py")
            t.bash("grep -n 'Baz(' c.py")
        self.assertEqual(fs.rank([t.runs()]), [])

    def test_one_occurrence_is_an_incident_not_a_shape(self):
        t = Transcript()
        t.typed("go").bash("git fetch").bash("git push")
        self.assertEqual(fs.rank([t.runs()]), [])

    def test_turns_outside_the_window_are_not_counted(self):
        t = Transcript()
        t.typed("go").bash("git fetch").bash("git push")
        t.typed("again").bash("git fetch").bash("git push")
        # The window opens after the first run's turns.
        since = sm.utc_stamp(t.records[7]["timestamp"])
        self.assertEqual(fs.rank([t.runs(since=since)]), [])


class Rendering(unittest.TestCase):
    def _shapes(self, n: int) -> list[fs.FlowShape]:
        t = Transcript()
        for i in range(n):
            for _ in range(2):
                t.typed("go").bash(f"git fetch{'' if i == 0 else ' ' + 'x' * i}")
                t.mcp(f"linear__step{chr(97 + i)}")
        return fs.rank([t.runs()])

    def test_the_default_table_is_the_top_n_with_an_omitted_line(self):
        shapes = self._shapes(fs.TOP_N + 2)
        self.assertEqual(len(shapes), fs.TOP_N + 2)
        lines = fs.table(shapes)
        self.assertEqual(sum(1 for line in lines if line.startswith("| `")), fs.TOP_N)
        self.assertIn("_+2 more flow shape(s) omitted", lines[-1])
        full = fs.table(shapes, None)
        self.assertEqual(
            sum(1 for line in full if line.startswith("| `")), fs.TOP_N + 2
        )

    def test_the_digest_section_carries_a_dotless_fingerprint(self):
        lines = fs.digest_section([worked_session().runs(), worked_session().runs()])
        prints = [line for line in lines if line.startswith("- `flow:")]
        self.assertTrue(prints)
        self.assertIn("flow:gh-auth-status-to-linear-get-issue-12", prints[0])
        self.assertIn("in 2 session(s)", prints[0])
        self.assertEqual(fs.digest_section([[]]), ["No flow shape recurred."])

    def test_shapes_sharing_their_ends_get_distinct_fingerprints(self):
        abc = fs.FlowShape(("git fetch", "git log", "git push"))
        adc = fs.FlowShape(("git fetch", "git diff", "git push"))
        self.assertNotEqual(fs.fingerprint(abc), fs.fingerprint(adc))
        self.assertEqual(
            fs.fingerprint(abc), fs.fingerprint(fs.FlowShape(abc.steps, occurrences=9))
        )
        self.assertNotIn(".", fs.fingerprint(abc))


class SessionMetricsWiring(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.home = pathlib.Path(self._tmp.name) / "claude"
        self.project = self.home / "projects" / "slug"
        self.project.mkdir(parents=True)

    def _write(self, sid: str, t: Transcript) -> None:
        (self.project / f"{sid}.jsonl").write_text(t.jsonl(), encoding="utf-8")

    def _run(self, argv: list[str]) -> str:
        buf, err = io.StringIO(), io.StringIO()
        with (
            mock.patch.dict(os.environ, {"CLAUDE_CONFIG_DIR": str(self.home)}),
            contextlib.redirect_stdout(buf),
            contextlib.redirect_stderr(err),
        ):
            self.assertEqual(sm.main(argv), 0)
        self.stderr = err.getvalue()
        return buf.getvalue()

    def test_one_session_ranks_nothing_across_sessions_ranks_the_bootstrap(self):
        self._write("one", worked_session())
        self._write("two", worked_session())
        alone = json.loads(self._run(["--session-id", "one", "--json"]))
        self.assertNotIn("flow_runs", alone)
        # Within one session the bootstrap ran once; only the noise recurs.
        self.assertTrue(
            all(len(f["steps"]) < len(BOOTSTRAP) for f in alone["flow_shapes"])
        )
        both = json.loads(
            self._run(
                ["--session-id", "one", "--json", "--flows-across", "two", "gone"]
            )
        )
        self.assertEqual(both["flow_sessions"], 2)
        self.assertEqual(len(both["flow_shapes"][0]["steps"]), len(BOOTSTRAP))
        self.assertEqual(both["flow_shapes"][0]["turns_left"], 2)
        self.assertIn("flows skip gone", self.stderr)
        text = self._run(["--session-id", "one", "--flows-across", "two"])
        self.assertIn("would save, across 2 sessions)", text)

    def test_markdown_carries_the_table_and_flows_all_lifts_the_cap(self):
        t = Transcript()
        for i in range(fs.TOP_N + 1):
            for _ in range(2):
                t.typed("go").bash("git fetch" + " x" * i).mcp(
                    f"linear__s{chr(97 + i)}"
                )
        self._write("many", t)
        text = self._run(["--session-id", "many"])
        self.assertIn("### Flow shapes", text)
        self.assertIn("_+1 more flow shape(s) omitted", text)
        full = self._run(["--session-id", "many", "--flows-all"])
        self.assertNotIn("more flow shape(s) omitted", full)
        data = json.loads(self._run(["--session-id", "many", "--json"]))
        self.assertEqual(
            (len(data["flow_shapes"]), data["flows_omitted"]), (fs.TOP_N, 1)
        )
        data = json.loads(self._run(["--session-id", "many", "--json", "--flows-all"]))
        self.assertEqual(
            (len(data["flow_shapes"]), data["flows_omitted"]), (fs.TOP_N + 1, 0)
        )


if __name__ == "__main__":
    unittest.main()
