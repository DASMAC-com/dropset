"""Stdlib ``unittest`` tests for ``ci_runs.py``.

The load-bearing properties: ``trigger`` makes one empty signed commit
without the skip trailer and pushes it under a lease pinned to the remote tip
read first; a locked signer refuses before any commit; ``cancel-stale``
reads the head after the runs and never cancels a run on that head. Every
``git`` / ``gh`` call goes through a scripted fake runner. Run via
``make tools-tests``.
"""

from __future__ import annotations

import io
import json
import unittest
from contextlib import redirect_stderr, redirect_stdout

import ci_runs

REPO = "o/r"
OLD, MID, HEAD, NEW = "a" * 40, "c" * 40, "b" * 40, "d" * 40


def pr_reply(sha, ref="eng-1"):
    return 0, json.dumps({"head": {"ref": ref, "sha": sha}}), ""


def runs_reply(*runs):
    return 0, json.dumps({"workflow_runs": list(runs)}), ""


def run(rid, sha, status="in_progress"):
    return {"id": rid, "head_sha": sha, "status": status}


class FakeRunner:
    """Answers each call from the first rule whose key is in the argv,
    consuming the rule's replies in order (the last one repeats)."""

    def __init__(self, rules):
        self.rules = [(key, list(replies)) for key, replies in rules]
        self.calls: list[list[str]] = []

    def __call__(self, argv):
        self.calls.append(argv)
        line = " ".join(argv)
        for key, replies in self.rules:
            if key in line:
                return replies.pop(0) if len(replies) > 1 else replies[0]
        raise AssertionError(f"unscripted call: {argv}")

    def cancelled(self):
        return [c[4].split("/")[-2] for c in self.calls if c[-1].endswith("/cancel")]

    def index(self, key):
        return next(i for i, c in enumerate(self.calls) if key in " ".join(c))


class CancelStaleTest(unittest.TestCase):
    def test_cancels_every_active_run_off_the_head(self):
        fake = FakeRunner(
            [
                ("/cancel", [(0, "", "")]),
                ("pulls/7", [pr_reply(HEAD)]),
                (
                    "actions/runs",
                    [
                        runs_reply(
                            run(1, OLD, "queued"),
                            run(2, MID),
                            run(3, HEAD),
                            run(4, OLD, "completed"),
                        )
                    ],
                ),
            ]
        )
        line = ci_runs.cancel_stale(fake, REPO, 7)
        self.assertEqual(fake.cancelled(), ["1", "2"])
        self.assertIn("cancelled 2 of 3 active", line)
        self.assertIn(f"head {HEAD[:12]} kept", line)

    def test_the_head_is_read_after_the_runs(self):
        # The head moved to NEW between the list and the head read: NEW's
        # run was not listed, so nothing on the current head can be touched,
        # and HEAD's listed run is now the stale one.
        fake = FakeRunner(
            [
                ("/cancel", [(0, "", "")]),
                ("pulls/7", [pr_reply(HEAD), pr_reply(NEW)]),
                ("actions/runs", [runs_reply(run(3, HEAD))]),
            ]
        )
        ci_runs.cancel_stale(fake, REPO, 7)
        pulls = [i for i, c in enumerate(fake.calls) if "pulls/7" in c[2]]
        self.assertLess(fake.index("actions/runs"), pulls[-1])
        self.assertEqual(fake.cancelled(), ["3"])

    def test_a_run_on_the_current_head_is_never_cancelled(self):
        fake = FakeRunner(
            [
                ("pulls/7", [pr_reply(HEAD)]),
                ("actions/runs", [runs_reply(run(3, HEAD), run(5, HEAD, "queued"))]),
            ]
        )
        line = ci_runs.cancel_stale(fake, REPO, 7)
        self.assertEqual(fake.cancelled(), [])
        self.assertIn("cancelled 0 of 2", line)

    def test_the_runs_read_is_scoped_to_the_branch_and_event(self):
        fake = FakeRunner(
            [("pulls/7", [pr_reply(HEAD)]), ("actions/runs", [runs_reply()])]
        )
        ci_runs.cancel_stale(fake, REPO, 7)
        listing = fake.calls[fake.index("actions/runs")]
        self.assertIn("branch=eng-1", listing)
        self.assertIn("event=pull_request", listing)

    def test_a_run_finished_before_its_cancel_is_not_a_failure(self):
        fake = FakeRunner(
            [
                ("/cancel", [(1, "", "gh: Cannot cancel (HTTP 409)")]),
                ("pulls/7", [pr_reply(HEAD)]),
                ("actions/runs", [runs_reply(run(1, OLD))]),
            ]
        )
        self.assertIn("cancelled 1 of 1", ci_runs.cancel_stale(fake, REPO, 7))

    def test_a_failed_cancel_refuses(self):
        fake = FakeRunner(
            [
                ("/cancel", [(1, "", "HTTP 403")]),
                ("pulls/7", [pr_reply(HEAD)]),
                ("actions/runs", [runs_reply(run(1, OLD))]),
            ]
        )
        with self.assertRaisesRegex(ci_runs.Refused, "cancel failed for run"):
            ci_runs.cancel_stale(fake, REPO, 7)

    def test_bad_replies_refuse(self):
        for rules in (
            [("pulls/7", [(1, "", "HTTP 404")])],
            [("pulls/7", [(0, "not json", "")])],
            [("pulls/7", [(0, "{}", "")])],
            [("pulls/7", [pr_reply(HEAD)]), ("actions/runs", [(0, "{}", "")])],
        ):
            with self.assertRaises(ci_runs.Refused):
                ci_runs.cancel_stale(FakeRunner(rules), REPO, 7)


def trigger_rules(
    remote=OLD,
    pr_ref="eng-1",
    commit=(0, "", ""),
    ancestor=0,
    staged=0,
    push=(0, "", ""),
    cancel=(0, "", ""),
):
    return [
        ("symbolic-ref", [(0, "eng-1\n", "")]),
        ("ls-remote", [(0, f"{remote}\trefs/heads/eng-1\n" if remote else "", "")]),
        ("merge-base --is-ancestor", [(ancestor, "", "")]),
        ("diff --cached --quiet", [(staged, "", "")]),
        ("git commit", [commit]),
        ("rev-parse HEAD", [(0, HEAD + "\n", "")]),
        ("git push", [push]),
        ("git reset", [(0, "", "")]),
        ("/cancel", [cancel]),
        ("pulls/7", [pr_reply(HEAD, pr_ref)]),
        ("actions/runs", [runs_reply(run(1, OLD), run(2, HEAD))]),
    ]


def no_commit_or_push(fake):
    return not any(c[:2] in (["git", "commit"], ["git", "push"]) for c in fake.calls)


SIGNED = (0, "signing-probe | signed | 0123 (dangling, no ref moved)")
LOCKED = (1, "signing-probe | LOCKED | unlock the op-ssh-sign backend | x")


class TriggerTest(unittest.TestCase):
    def test_commits_signed_without_the_trailer_pushes_and_cancels(self):
        fake = FakeRunner(trigger_rules())
        line = ci_runs.trigger(fake, REPO, 7, probe=lambda: SIGNED)
        commit = fake.calls[fake.index("git commit")]
        self.assertEqual(
            commit,
            [
                "git",
                "commit",
                "--allow-empty",
                "-S",
                "-q",
                "-m",
                ci_runs.TRIGGER_SUBJECT,
            ],
        )
        self.assertNotIn("skip-checks", " ".join(commit))
        # A plain fast-forward: never a force of any kind.
        push = fake.calls[fake.index("git push")]
        self.assertEqual(push, ["git", "push", "-q", "origin", "eng-1"])
        ancestry = fake.calls[fake.index("merge-base")]
        self.assertEqual(ancestry[-2:], [OLD, "HEAD"])
        self.assertLess(fake.index("merge-base"), fake.index("git commit"))
        self.assertLess(fake.index("git push"), fake.index("actions/runs"))
        self.assertEqual(fake.cancelled(), ["1"])
        self.assertTrue(line.startswith(f"triggered {HEAD[:12]} on origin/eng-1"))

    def test_a_locked_signer_refuses_before_any_commit(self):
        fake = FakeRunner(trigger_rules())
        with self.assertRaisesRegex(ci_runs.Refused, "LOCKED"):
            ci_runs.trigger(fake, REPO, 7, probe=lambda: LOCKED)
        joined = [" ".join(c) for c in fake.calls]
        self.assertFalse(any("git commit" in c or "git push" in c for c in joined))

    def test_another_branch_refuses(self):
        fake = FakeRunner(trigger_rules(pr_ref="eng-2"))
        with self.assertRaisesRegex(ci_runs.Refused, "heads `eng-2`"):
            ci_runs.trigger(fake, REPO, 7, probe=lambda: SIGNED)

    def test_an_unpushed_branch_refuses(self):
        fake = FakeRunner(trigger_rules(remote=""))
        with self.assertRaisesRegex(ci_runs.Refused, "does not exist"):
            ci_runs.trigger(fake, REPO, 7, probe=lambda: SIGNED)

    def test_a_failed_commit_refuses_before_the_push(self):
        fake = FakeRunner(trigger_rules(commit=(1, "", "fatal: boom")))
        with self.assertRaisesRegex(ci_runs.Refused, "boom"):
            ci_runs.trigger(fake, REPO, 7, probe=lambda: SIGNED)
        self.assertFalse(any(c[:2] == ["git", "push"] for c in fake.calls))

    def test_a_remote_ahead_of_this_checkout_refuses_before_any_commit(self):
        fake = FakeRunner(trigger_rules(ancestor=1))
        with self.assertRaisesRegex(ci_runs.Refused, "lacks; pull or rebase"):
            ci_runs.trigger(fake, REPO, 7, probe=lambda: SIGNED)
        self.assertTrue(no_commit_or_push(fake))

    def test_a_staged_index_refuses_before_any_commit(self):
        fake = FakeRunner(trigger_rules(staged=1))
        with self.assertRaisesRegex(ci_runs.Refused, "staged changes"):
            ci_runs.trigger(fake, REPO, 7, probe=lambda: SIGNED)
        self.assertTrue(no_commit_or_push(fake))

    def test_a_failed_push_drops_the_trigger_commit(self):
        fake = FakeRunner(trigger_rules(push=(1, "", "rejected (non-fast-forward)")))
        with self.assertRaisesRegex(ci_runs.Refused, "non-fast-forward"):
            ci_runs.trigger(fake, REPO, 7, probe=lambda: SIGNED)
        reset = fake.calls[fake.index("git reset")]
        self.assertEqual(reset, ["git", "reset", "-q", "--soft", "HEAD~1"])
        self.assertLess(fake.index("git push"), fake.index("git reset"))
        self.assertFalse(any("actions/runs" in " ".join(c) for c in fake.calls))

    def test_a_failed_cancel_after_the_push_still_reports_the_trigger(self):
        fake = FakeRunner(trigger_rules(cancel=(1, "", "HTTP 403")))
        line = ci_runs.trigger(fake, REPO, 7, probe=lambda: SIGNED)
        self.assertTrue(line.startswith(f"triggered {HEAD[:12]}"), line)
        self.assertIn("cancel-stale failed", line)
        self.assertFalse(any(c[:2] == ["git", "reset"] for c in fake.calls))


class HeadLagTest(unittest.TestCase):
    def test_waits_for_the_pushed_head_then_cancels_the_old(self):
        sleeps = []
        fake = FakeRunner(
            [
                ("/cancel", [(0, "", "")]),
                # ref read, then two lagging head reads, then the pushed head
                (
                    "pulls/7",
                    [pr_reply(OLD), pr_reply(OLD), pr_reply(OLD), pr_reply(NEW)],
                ),
                ("actions/runs", [runs_reply(run(1, OLD), run(2, NEW))]),
            ]
        )
        line = ci_runs.cancel_stale(fake, REPO, 7, expect=NEW, sleep=sleeps.append)
        self.assertEqual(len(sleeps), 2)
        self.assertEqual(fake.cancelled(), ["1"])
        self.assertIn(f"head {NEW[:12]} kept", line)

    def test_a_head_that_never_catches_up_keeps_both(self):
        sleeps = []
        fake = FakeRunner(
            [
                ("pulls/7", [pr_reply(OLD)]),
                ("actions/runs", [runs_reply(run(1, OLD), run(2, NEW))]),
            ]
        )
        ci_runs.cancel_stale(fake, REPO, 7, expect=NEW, sleep=sleeps.append)
        self.assertEqual(len(sleeps), ci_runs.HEAD_WAIT_TRIES)
        # A lagging read never cancels the run on the commit just pushed.
        self.assertEqual(fake.cancelled(), [])


class MainTest(unittest.TestCase):
    def call(self, argv, fake, probe=lambda: SIGNED):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = ci_runs.main(argv, runner=fake, probe=probe)
        return code, out.getvalue(), err.getvalue()

    def test_exit_codes_and_one_verdict_line(self):
        code, out, _ = self.call(["trigger", "--pr", "7"], FakeRunner(trigger_rules()))
        self.assertEqual(code, 0)
        self.assertEqual(len(out.splitlines()), 1)
        self.assertTrue(out.startswith("ci-runs | triggered"))
        code, _, err = self.call(
            ["trigger", "--pr", "7"], FakeRunner(trigger_rules()), probe=lambda: LOCKED
        )
        self.assertEqual(code, 1)
        self.assertTrue(err.startswith("ci-runs | refused | signing-probe | LOCKED"))
        code, out, _ = self.call(
            ["cancel-stale", "--pr", "7", "--repo", REPO],
            FakeRunner(
                [("pulls/7", [pr_reply(HEAD)]), ("actions/runs", [runs_reply()])]
            ),
        )
        self.assertEqual(code, 0)
        self.assertIn("cancelled 0 of 0", out)

    def test_usage_is_exit_2(self):
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as caught:
            ci_runs.main(["trigger"], runner=FakeRunner([]))
        self.assertEqual(caught.exception.code, 2)


if __name__ == "__main__":
    unittest.main()
