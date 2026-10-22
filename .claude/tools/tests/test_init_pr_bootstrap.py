"""Tests for ``init_pr_bootstrap.py`` — the one-run init-pr bootstrap.

Every external command goes through an injected runner, so these drive the
step sequence, the stop-before-mutation gates and the idempotent re-run
without touching git, gh or Linear.
"""

from __future__ import annotations

import json
import tempfile
import unittest

import init_pr_bootstrap as boot

HELPER = {
    "tag": "eng-42",
    "tag_valid": True,
    "current_branch": "eng-42",
    "normalized_branch": "eng-42",
    "rename_needed": False,
    "env_link": "created",
    "secrets_env_link": "exists",
    "frontend_node_modules": "present",
    "program_so": "absent",
    "signing": "external-signer",
}

PR_URL = "https://github.com/DASMAC-com/dropset/pull/700"


class FakeRunner:
    """Answer each command by its first matching prefix; record every call."""

    def __init__(self, overrides=None, helper=None, prs=()):
        self.calls: list[list[str]] = []
        helper_json = json.dumps({**HELPER, **(helper or {})})
        self.table = [
            (
                ["gh", "auth", "status"],
                (0, "Token scopes: 'notifications', 'repo'", ""),
            ),
            (["gh", "pr", "list"], (0, json.dumps(list(prs)), "")),
            (["init_pr_branch.py"], (0, helper_json, "")),
            (["git", "fetch"], (0, "", "")),
            (["git", "branch", "-m"], (0, "", "")),
            (["git", "rebase", "origin/main"], (0, "up to date", "")),
            (["git", "rebase", "--abort"], (0, "", "")),
            (["pnpm"], (0, "installed", "")),
            (["git", "log"], (0, "", "")),
            (["git", "commit"], (0, "", "")),
            (["git", "push"], (0, "", "")),
            (["gh", "pr", "create"], (0, PR_URL + "\n", "")),
            (["gh", "pr", "view"], (0, json.dumps({"id": "PR_node"}), "")),
            (
                ["gh", "api", "graphql"],
                (0, '{"viewerSubscription":"UNSUBSCRIBED"}', ""),
            ),
            (["linear_patch.py", "state"], (0, "SET ENG-42", "")),
            (["linear_patch.py", "read"], (0, "WROTE 1234 bytes", "")),
        ]
        for prefix, reply in (overrides or {}).items():
            self.table.insert(0, (list(prefix), reply))

    def _key(self, cmd):
        # Script paths are absolute; match on the basename.
        if len(cmd) > 1 and cmd[1].endswith(".py"):
            return [cmd[1].rsplit("/", 1)[-1], *cmd[2:]]
        return cmd

    def __call__(self, cmd, timeout):
        self.calls.append(cmd)
        key = self._key(cmd)
        for prefix, reply in self.table:
            if key[: len(prefix)] == prefix:
                return reply
        raise AssertionError(f"unexpected command: {cmd}")

    def ran(self, *prefix):
        return any(self._key(c)[: len(prefix)] == list(prefix) for c in self.calls)


def bootstrap(runner, tag="42", spec_out=None, allow_merged=False):
    log_dir = tempfile.mkdtemp()
    return boot.Bootstrap(tag, spec_out, allow_merged, run=runner, log_dir=log_dir)


def statuses(result):
    return {s["step"]: s["status"] for s in result["steps"]}


class HappyPathTests(unittest.TestCase):
    def test_a_clean_run_completes_every_step(self):
        runner = FakeRunner()
        result = bootstrap(runner, spec_out="/tmp/spec.md").execute()
        self.assertIsNone(result["stopped_at"])
        self.assertEqual(result["remaining"], [])
        self.assertEqual([s["step"] for s in result["steps"]], list(boot.STEPS))
        self.assertEqual(result["pr"], {"number": 700, "url": PR_URL})
        self.assertEqual(result["spec_path"], "/tmp/spec.md")
        self.assertEqual(result["tag"], "eng-42")
        self.assertEqual(result["facts"]["program_so"], "absent")

    def test_the_commit_subject_matches_the_pr_title(self):
        runner = FakeRunner()
        bootstrap(runner).execute()
        subject = "chore(ENG-42): Bootstrap the worktree"
        commit = next(c for c in runner.calls if c[:2] == ["git", "commit"])
        create = next(c for c in runner.calls if c[:3] == ["gh", "pr", "create"])
        self.assertIn("-S", commit)
        self.assertEqual(commit[-1], subject)
        self.assertEqual(create[create.index("--title") + 1], subject)
        self.assertIn("--draft", create)

    def test_install_runs_only_when_node_modules_is_absent(self):
        runner = FakeRunner()
        bootstrap(runner).execute()
        self.assertFalse(runner.ran("pnpm"))
        runner = FakeRunner(helper={"frontend_node_modules": "absent"})
        result = bootstrap(runner).execute()
        self.assertTrue(runner.ran("pnpm"))
        self.assertEqual(statuses(result)["install"], "ok")

    def test_rename_runs_only_when_needed(self):
        runner = FakeRunner(
            helper={"rename_needed": True, "current_branch": "worktree-42"}
        )
        result = bootstrap(runner).execute()
        self.assertTrue(runner.ran("git", "branch", "-m", "worktree-42", "eng-42"))
        self.assertEqual(statuses(result)["rename"], "ok")


class StopBeforeMutationTests(unittest.TestCase):
    def assertNoMutation(self, runner):
        for prefix in (("git", "fetch"), ("git", "commit"), ("gh", "pr", "create")):
            self.assertFalse(runner.ran(*prefix), prefix)

    def test_a_dead_credential_stops_first(self):
        runner = FakeRunner({("gh", "auth", "status"): (1, "", "not logged in")})
        result = bootstrap(runner).execute()
        self.assertEqual(result["stopped_at"], "auth")
        self.assertEqual(result["remaining"], list(boot.STEPS[1:]))
        self.assertNoMutation(runner)

    def test_a_merged_pr_stops_unless_allowed(self):
        prs = [{"number": 9, "state": "MERGED", "headRefName": "eng-42"}]
        runner = FakeRunner(prs=prs)
        result = bootstrap(runner).execute()
        self.assertEqual(result["stopped_at"], "merged_check")
        self.assertIn("#9", result["steps"][-1]["detail"])
        self.assertNoMutation(runner)
        result = bootstrap(FakeRunner(prs=prs), allow_merged=True).execute()
        self.assertIsNone(result["stopped_at"])

    def test_an_invalid_tag_stops_before_any_read(self):
        runner = FakeRunner()
        result = bootstrap(runner, tag="banana").execute()
        self.assertEqual(result["stopped_at"], "merged_check")
        self.assertFalse(runner.ran("gh", "pr", "list"))

    def test_a_locked_signer_stops_before_the_rename(self):
        for signing in boot.SIGNING_STOPS:
            with self.subTest(signing=signing):
                runner = FakeRunner(helper={"signing": signing})
                result = bootstrap(runner).execute()
                self.assertEqual(result["stopped_at"], "branch")
                self.assertNoMutation(runner)

    def test_a_missing_notifications_scope_is_a_note_not_a_stop(self):
        runner = FakeRunner({("gh", "auth", "status"): (0, "Token scopes: 'repo'", "")})
        result = bootstrap(runner).execute()
        self.assertIsNone(result["stopped_at"])
        self.assertTrue(any("notifications" in n for n in result["notes"]))


class MidRunFailureTests(unittest.TestCase):
    def test_a_rebase_conflict_aborts_and_names_what_remains(self):
        runner = FakeRunner({("git", "rebase", "origin/main"): (1, "", "CONFLICT")})
        result = bootstrap(runner).execute()
        self.assertEqual(result["stopped_at"], "rebase")
        self.assertTrue(runner.ran("git", "rebase", "--abort"))
        self.assertEqual(result["remaining"][0], "install")
        self.assertFalse(runner.ran("git", "commit"))

    def test_a_signing_failure_stops_before_the_push(self):
        runner = FakeRunner({("git", "commit"): (1, "", "failed to fill whole buffer")})
        result = bootstrap(runner).execute()
        self.assertEqual(result["stopped_at"], "commit")
        self.assertIn("1Password", result["steps"][-1]["detail"])
        self.assertFalse(runner.ran("git", "push"))

    def test_best_effort_steps_fail_without_stopping(self):
        runner = FakeRunner(
            {
                ("gh", "api", "graphql"): (1, "", "INSUFFICIENT_SCOPES"),
                ("linear_patch.py", "state"): (1, "", "HTTP 401"),
                ("pnpm",): (1, "", "ERR"),
            },
            helper={"frontend_node_modules": "absent"},
        )
        result = bootstrap(runner, spec_out="/tmp/spec.md").execute()
        self.assertIsNone(result["stopped_at"])
        got = statuses(result)
        self.assertEqual(
            (got["mute"], got["linear_state"], got["install"]),
            ("failed", "failed", "failed"),
        )
        self.assertEqual(got["spec"], "ok")


class ResumeTests(unittest.TestCase):
    def test_a_rerun_reuses_the_commit_and_the_open_pr(self):
        runner = FakeRunner(
            {("git", "log"): (0, "chore(ENG-42): Bootstrap the worktree\n", "")},
            prs=[
                {"number": 700, "state": "OPEN", "headRefName": "eng-42", "url": PR_URL}
            ],
        )
        result = bootstrap(runner).execute()
        got = statuses(result)
        self.assertEqual((got["commit"], got["pr"]), ("skipped", "skipped"))
        self.assertFalse(runner.ran("git", "commit"))
        self.assertFalse(runner.ran("gh", "pr", "create"))
        self.assertEqual(result["pr"]["number"], 700)

    def test_an_open_pr_on_another_branch_is_not_reused(self):
        runner = FakeRunner(
            prs=[{"number": 5, "state": "OPEN", "headRefName": "eng-41", "url": "x"}]
        )
        result = bootstrap(runner).execute()
        self.assertEqual(result["pr"]["number"], 700)

    def test_the_push_carries_a_lease_so_a_rebased_rerun_lands(self):
        runner = FakeRunner()
        bootstrap(runner).execute()
        push = next(c for c in runner.calls if c[:2] == ["git", "push"])
        self.assertIn("--force-with-lease", push)


class MergedMatchTests(unittest.TestCase):
    def test_a_merged_pr_that_only_mentions_the_tag_is_ignored(self):
        prs = [
            {"number": 8, "state": "MERGED", "headRefName": "eng-7", "title": "x"},
            {"number": 9, "state": "CLOSED", "headRefName": "eng-7", "title": "y"},
        ]
        result = bootstrap(FakeRunner(prs=prs)).execute()
        self.assertIsNone(result["stopped_at"])
        self.assertEqual(result["notes"], [])

    def test_a_merged_pr_scoped_to_the_issue_stops_from_any_branch(self):
        prs = [
            {
                "number": 8,
                "state": "MERGED",
                "headRefName": "other",
                "title": "fix(ENG-42): Thing",
            }
        ]
        result = bootstrap(FakeRunner(prs=prs)).execute()
        self.assertEqual(result["stopped_at"], "merged_check")


class UnexpectedReplyTests(unittest.TestCase):
    def test_a_non_json_mute_reply_fails_the_step_not_the_run(self):
        runner = FakeRunner({("gh", "pr", "view"): (0, "not json", "")})
        result = bootstrap(runner).execute()
        self.assertIsNone(result["stopped_at"])
        mute = next(s for s in result["steps"] if s["step"] == "mute")
        self.assertEqual(mute["status"], "failed")
        self.assertIn("JSONDecodeError", mute["detail"])

    def test_a_non_json_pr_list_stops_with_a_manifest(self):
        runner = FakeRunner({("gh", "pr", "list"): (0, "<html>", "")})
        result = bootstrap(runner).execute()
        self.assertEqual(result["stopped_at"], "merged_check")
        self.assertFalse(runner.ran("git", "fetch"))

    def test_a_helper_reply_that_is_not_an_object_stops(self):
        for reply in ("[]", json.dumps({**HELPER, "normalized_branch": ""})):
            with self.subTest(reply=reply):
                runner = FakeRunner({("init_pr_branch.py",): (0, reply, "")})
                result = bootstrap(runner).execute()
                self.assertEqual(result["stopped_at"], "branch")
                self.assertFalse(runner.ran("git", "fetch"))


if __name__ == "__main__":
    unittest.main()
